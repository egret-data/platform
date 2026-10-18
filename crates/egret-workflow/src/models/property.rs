// kestra: core/src/main/java/io/kestra/core/models/property/Property.java

use std::marker::PhantomData;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::runners::env::{render_str, RenderContext, RenderError};

/// A plugin property: raw source that is rendered and converted to `T` at use
/// time (W2), never at parse time.
///
/// `T` is a marker only. It records what the field will become so callers read
/// as documentation and so W2's `render` can pick a conversion, but it holds no
/// value of that type -- Kestra's `Property<T>` works the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Property<T> {
    source: String,
    _target: PhantomData<T>,
}

impl<T> Property<T> {
    pub fn from_source(source: String) -> Self {
        Self {
            source,
            _target: PhantomData,
        }
    }

    pub fn source(&self) -> &str {
        &self.source
    }
}

impl<T: DeserializeOwned> Property<T> {
    /// Renders this property's source against `cx` and converts the result to
    /// `T`, in one step.
    ///
    /// One step on purpose: this is Kestra's
    /// `runContext.render(prop).as(Class)`, where the rendered text is never
    /// exposed as an intermediate a caller could accidentally use untyped. A
    /// `Property<i64>` that renders to `"abc"` fails here, at the seam that
    /// knows what it was meant to be, rather than at whatever tries to parse
    /// it later.
    ///
    /// # Conversion rule
    ///
    /// The rendered text is offered to serde twice, in this order:
    ///
    /// 1. **as a string** — `T` gets the rendered text verbatim. This is what
    ///    a `Property<String>` takes, and it is first so that a `String` whose
    ///    rendered value happens to look like JSON (`"42"`, `"true"`,
    ///    `"[1,2]"`) stays that text rather than being silently parsed and
    ///    re-stringified.
    /// 2. **as JSON** — only if step 1's `T` will not accept a string. This is
    ///    what a `Property<i64>`, `Property<bool>` or `Property<Vec<String>>`
    ///    takes, and it matches the deserializer's own rule in reverse:
    ///    `Property` stores a sequence or mapping as its JSON text, so that is
    ///    the text a structured `T` has to parse back.
    ///
    /// The one shape this rule cannot serve is `Property<serde_json::Value>`,
    /// which accepts a string and therefore always takes step 1 — a rendered
    /// `{"a":1}` comes back as a JSON *string*, not an object. That is
    /// deliberate rather than accidental: `serde_json::Value` names no target
    /// shape, so there is nothing to prefer. Ask for the shape you want.
    ///
    /// # Identity
    ///
    /// A source with no `{` renders to itself (Kestra's own short-circuit,
    /// `VariableRenderer.java:76-79`). W5's plugin host pre-renders every
    /// property before handing a task its arguments; without that identity,
    /// pre-rendering would not be a no-op for the properties that are already
    /// literal, and running it twice would not be safe.
    pub fn render(&self, cx: &RenderContext) -> Result<T, RenderError> {
        let rendered = render_str(&self.source, cx)?;

        match serde_json::from_value::<T>(serde_json::Value::String(rendered.clone())) {
            Ok(value) => Ok(value),
            Err(as_string) => serde_json::from_str::<T>(&rendered).map_err(|as_json| {
                RenderError::Convert {
                    target: std::any::type_name::<T>(),
                    rendered,
                    // Both complaints, because either one alone reads as the
                    // wrong diagnosis: "invalid type: string" is confusing for
                    // a `Vec<String>` target, and "expected value" is
                    // confusing for a `String` one.
                    detail: format!("{as_string}; and as JSON: {as_json}"),
                }
            }),
        }
    }
}

impl<T> Serialize for Property<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.source)
    }
}

impl<'de, T> Deserialize<'de> for Property<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Deserialize to an untyped value first, then flatten to source text
        // the way Kestra's PropertyDeserializer does: a scalar keeps its string
        // form, a sequence or mapping is re-serialised to JSON.
        let v = serde_json::Value::deserialize(d)?;
        let source = match v {
            serde_json::Value::String(s) => s,
            // Kestra's PropertyDeserializer has no `getNullValue()` override, so
            // Jackson never flattens a null -- the field is simply left null.
            // There is no source text to invent for that: a `String::new()`
            // fallback here would let a null reach W2's renderer as `""`, a
            // silent wrong value instead of an error. A field that may be
            // absent should be `Option<Property<T>>`, which short-circuits
            // before this branch is ever reached.
            serde_json::Value::Null => {
                return Err(serde::de::Error::custom(
                    "Property<T> has no null representation (Kestra's PropertyDeserializer \
                     never flattens a null); use Option<Property<T>> to allow this field to be \
                     absent instead",
                ));
            }
            other => other.to_string(),
        };
        Ok(Self::from_source(source))
    }
}

/// Kestra's `Property.asMap` (`Property.java:264-287`) over a property's raw
/// value, as `runContext.render(values).asMap(String.class, Object.class)`
/// reads it (`OutputValues.java:71`). Group C1 Task 2; C2's `SetVariables` and
/// `Labels` reuse it.
///
/// - A **string** that, trimmed, starts with an opening Pebble delimiter
///   (`{{` or `{%`) and ends with a closing one (`}}` or `%}`) --
///   `PebbleUtil.startsWithOpeningBlockDelimiter` /
///   `endsWithClosingBlockDelimiter` -- is rendered whole and its text read
///   as a JSON object.
/// - Any **other string** is read as a JSON object first
///   (`MAPPER.readValue(expression, Map.class)`), then walked as below.
/// - An **object** is walked as `VariableRenderer.render(Map)` does
///   (`VariableRenderer.java:187-221`): every key is rendered, every string
///   leaf is rendered to a string, arrays and objects are walked, and
///   numbers, booleans and null are kept. Two keys that render to the same
///   text keep the FIRST (`putIfAbsent`), unless the first is null, which
///   `putIfAbsent` replaces.
/// - Anything else (an array, a number, a boolean, null) is not a map: an
///   error, as Jackson's `readValue(…, Map.class)` throws on its text.
///
/// `field` is the property's name, the prefix of every error:
/// `` `values.<key>`: <render error> ``. The output keeps source order
/// (`serde_json`'s `preserve_order`, Kestra's `LinkedHashMap`).
pub fn render_map(
    raw: &serde_json::Value,
    field: &str,
    cx: &RenderContext,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    use serde_json::Value;
    match raw {
        Value::Object(map) => render_object(map, field, cx),
        Value::String(source) => {
            let trimmed = source.trim();
            let whole = (trimmed.starts_with("{{") || trimmed.starts_with("{%"))
                && (trimmed.ends_with("}}") || trimmed.ends_with("%}"));
            if whole {
                let text = render_str(source, cx).map_err(|e| format!("`{field}`: {e}"))?;
                match serde_json::from_str::<Value>(&text) {
                    Ok(Value::Object(map)) => Ok(map),
                    _ => Err(format!(
                        "`{field}`: the expression rendered `{text}`, which is not a JSON object"
                    )),
                }
            } else {
                match serde_json::from_str::<Value>(source) {
                    Ok(Value::Object(map)) => render_object(&map, field, cx),
                    _ => Err(format!("`{field}`: `{source}` is not a JSON object")),
                }
            }
        }
        other => Err(format!(
            "`{field}`: expected a map, got {}",
            match other {
                Value::Array(_) => "an array",
                Value::Number(_) => "a number",
                Value::Bool(_) => "a boolean",
                _ => "null",
            }
        )),
    }
}

/// [`render_map`]'s walk of one object, at `path`.
fn render_object(
    map: &serde_json::Map<String, serde_json::Value>,
    path: &str,
    cx: &RenderContext,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let mut out = serde_json::Map::with_capacity(map.len());
    for (raw_key, value) in map {
        let at = format!("{path}.{raw_key}");
        let key = render_str(raw_key, cx).map_err(|e| format!("`{at}`: {e}"))?;
        let value = render_value(value, &at, cx)?;
        // Java's `putIfAbsent` (`VariableRenderer.java:194`): a key already
        // there is kept, unless it is mapped to null.
        match out.get_mut(&key) {
            Some(existing) if existing.is_null() => *existing = value,
            Some(_) => {}
            None => {
                out.insert(key, value);
            }
        }
    }
    Ok(out)
}

/// `VariableRenderer.renderObject`: strings rendered, arrays and objects
/// walked, every other scalar kept.
fn render_value(
    value: &serde_json::Value,
    path: &str,
    cx: &RenderContext,
) -> Result<serde_json::Value, String> {
    use serde_json::Value;
    Ok(match value {
        Value::String(s) => Value::String(render_str(s, cx).map_err(|e| format!("`{path}`: {e}"))?),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .enumerate()
                .map(|(i, v)| render_value(v, &format!("{path}[{i}]"), cx))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(map) => Value::Object(render_object(map, path, cx)?),
        other => other.clone(),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{render_map, Property};
    use crate::runners::env::{RenderContext, RenderError};

    fn cx(value: serde_json::Value) -> RenderContext {
        RenderContext::from_json_object(&value).expect("test context must be a JSON object")
    }

    /// **The identity W5's plugin host depends on.** A property with no
    /// expression in it renders to exactly its own source -- so pre-rendering
    /// every property before a task runs is a no-op for the literal ones, and
    /// is safe to do more than once.
    ///
    /// Note the short-circuit Kestra actually uses is `{`, not `{{`: the
    /// `{` case below would otherwise reach the engine and fail to parse.
    #[test]
    fn a_source_without_an_expression_renders_to_itself() {
        let ctx = cx(json!({ "x": "SHOULD-NOT-APPEAR" }));
        for source in [
            "hello",
            "",
            "a plain sentence with punctuation: 1, 2, 3.",
            "100%",
            // Kestra syntax that never reaches the rewriter, because there is
            // no `{` to make this a template in the first place.
            "a ?? b",
            "flag ? 'yes' : 'no'",
        ] {
            let p: Property<String> = Property::from_source(source.to_string());
            assert_eq!(p.render(&ctx).unwrap(), source, "source: {source:?}");
        }
    }

    /// A `{` alone is enough to send a source through the engine, which is
    /// Kestra's rule verbatim -- worth pinning separately from the identity
    /// above, because the obvious short-circuit (`{{`) would take a different
    /// branch here and nothing else in the suite would notice.
    #[test]
    fn a_lone_brace_still_goes_through_the_engine() {
        let p: Property<String> = Property::from_source("{ \"json\": 1 }".to_string());
        assert_eq!(p.render(&cx(json!({}))).unwrap(), "{ \"json\": 1 }");
    }

    /// Rendering and conversion are one step: the target type is what decides
    /// how the rendered text is read.
    #[test]
    fn render_converts_to_the_target_type() {
        let ctx = cx(json!({ "n": 7, "flag": true, "name": "ada", "items": ["a", "b"] }));

        let s: Property<String> = Property::from_source("{{ name }}".to_string());
        assert_eq!(s.render(&ctx).unwrap(), "ada");

        let i: Property<i64> = Property::from_source("{{ n }}".to_string());
        assert_eq!(i.render(&ctx).unwrap(), 7);

        let b: Property<bool> = Property::from_source("{{ flag ? 'true' : 'false' }}".to_string());
        assert!(b.render(&ctx).unwrap());

        let v: Property<Vec<String>> = Property::from_source("[\"x\", \"{{ name }}\"]".to_string());
        assert_eq!(
            v.render(&ctx).unwrap(),
            vec!["x".to_string(), "ada".to_string()]
        );

        // A `String` target keeps JSON-looking text as text rather than
        // parsing it -- the ordering rule in `render`'s doc.
        let looks_like_json: Property<String> = Property::from_source("{{ n }}".to_string());
        assert_eq!(looks_like_json.render(&ctx).unwrap(), "7");
    }

    /// A wrong-typed render fails at the seam that knows the target, naming
    /// both the text and the type.
    #[test]
    fn a_render_that_is_not_the_target_type_is_refused() {
        let p: Property<i64> = Property::from_source("{{ name }}".to_string());
        let err = p.render(&cx(json!({ "name": "ada" }))).unwrap_err();
        match err {
            RenderError::Convert {
                target, rendered, ..
            } => {
                assert!(target.contains("i64"), "target: {target}");
                assert_eq!(rendered, "ada");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    /// A render failure is not swallowed into a conversion failure: a missing
    /// variable must surface as a template error, or an author debugging a
    /// typo is told their value is the wrong type instead.
    #[test]
    fn a_missing_variable_surfaces_as_a_template_error() {
        let p: Property<String> = Property::from_source("{{ nothere }}".to_string());
        let err = p.render(&cx(json!({}))).unwrap_err();
        assert!(
            matches!(err, RenderError::Template(_)),
            "unexpected error: {err:?}"
        );
    }

    /// Kestra's PropertyDeserializer stores EVERYTHING as a string: scalars
    /// become their string form, and arrays/objects are re-serialised to JSON
    /// text. Reproducing that is the whole point of this type -- a Property is
    /// raw source, rendered and converted at use time (W2), not a parsed value.
    #[test]
    fn every_yaml_shape_becomes_source_text() {
        let scalar: Property<String> = serde_norway::from_str("hello").unwrap();
        assert_eq!(scalar.source(), "hello");

        let number: Property<i64> = serde_norway::from_str("42").unwrap();
        assert_eq!(number.source(), "42");

        let expression: Property<String> = serde_norway::from_str("\"{{ inputs.x }}\"").unwrap();
        assert_eq!(expression.source(), "{{ inputs.x }}");

        // A sequence is re-serialised to JSON, exactly as Java does.
        let list: Property<Vec<String>> = serde_norway::from_str("[a, b]").unwrap();
        assert_eq!(list.source(), r#"["a","b"]"#);

        let map: Property<serde_json::Value> = serde_norway::from_str("{k: v}").unwrap();
        assert_eq!(map.source(), r#"{"k":"v"}"#);
    }

    /// Round-tripping must not quote a source that was not quoted, or a
    /// published snapshot would stop matching the author's YAML.
    #[test]
    fn serialises_back_to_its_source() {
        let p: Property<String> = serde_norway::from_str("hello").unwrap();
        assert_eq!(serde_json::to_string(&p).unwrap(), "\"hello\"");
    }

    /// Round 1 review: Kestra's `PropertyDeserializer` has no `getNullValue()`
    /// override, so Jackson never flattens a null into a `Property` -- the
    /// field is simply left null. A bare (non-`Option`) `Property<T>` field
    /// must therefore reject `null` rather than inventing an empty-string
    /// source, which would silently reach W2's renderer as `""`.
    #[test]
    fn bare_property_rejects_null() {
        let err = serde_norway::from_str::<Property<String>>("null").unwrap_err();
        assert!(err.to_string().contains("null"), "unexpected error: {err}");
    }
    /// `render_map` (group C1 Task 2): Kestra's `Property.asMap` over a map
    /// source walks it with `VariableRenderer.render(Map)`
    /// (`VariableRenderer.java:187-221`): keys rendered, string leaves
    /// rendered to strings, lists and maps walked, other scalars kept.
    #[test]
    fn render_map_renders_keys_and_string_leaves_and_keeps_other_scalars() {
        let mut cx = RenderContext::new();
        cx.insert("n", json!(2)).insert("k", json!("key"));
        let raw = json!({"a": "{{ n + 1 }}", "{{ k }}": [1, "{{ n }}", {"x": true}], "z": null});
        let out = render_map(&raw, "values", &cx).unwrap();
        assert_eq!(
            serde_json::Value::Object(out),
            json!({"a": "3", "key": [1, "2", {"x": true}], "z": null})
        );
    }

    /// A string source that is one whole expression is rendered and its text
    /// read as a JSON object (`Property.java:274-276`); text that is not one
    /// is an error, not an empty map.
    #[test]
    fn render_map_parses_a_whole_expression_as_a_json_object() {
        let mut cx = RenderContext::new();
        cx.insert("m", json!({"p": 1}));
        let out = render_map(&json!("{{ m | toJson }}"), "values", &cx).unwrap();
        assert_eq!(serde_json::Value::Object(out), json!({"p": 1}));
        let err = render_map(&json!("{{ 'not a map' }}"), "values", &cx).unwrap_err();
        assert!(
            err.starts_with(
                "`values`: the expression rendered `not a map`, which is not a JSON object"
            ),
            "{err}"
        );
    }

    /// `map.putIfAbsent(key, value)` (`VariableRenderer.java:194`): when two
    /// keys render alike, the first in source order wins.
    #[test]
    fn render_map_keeps_the_first_of_two_keys_that_render_alike() {
        let mut cx = RenderContext::new();
        cx.insert("k", json!("a"));
        let raw = json!({"a": "first", "{{ k }}": "second"});
        assert_eq!(
            render_map(&raw, "values", &cx).unwrap()["a"],
            json!("first")
        );
    }

    /// Java's `Map.putIfAbsent` also replaces a key "mapped to null", so a
    /// later key that renders alike takes the place of an earlier null.
    #[test]
    fn render_map_replaces_an_earlier_null_with_a_later_key_that_renders_alike() {
        let mut cx = RenderContext::new();
        cx.insert("k", json!("a"));
        let raw = json!({"a": null, "{{ k }}": "x"});
        assert_eq!(
            serde_json::Value::Object(render_map(&raw, "values", &cx).unwrap()),
            json!({"a": "x"})
        );
    }

    /// A string that is not an expression is read as a JSON object
    /// (`MAPPER.readValue(expression, Map.class)`, `Property.java:280`) and
    /// then walked; anything else that is not an object is an error.
    #[test]
    fn render_map_reads_a_literal_string_as_a_json_object_or_refuses_it() {
        let mut cx = RenderContext::new();
        cx.insert("n", json!(5));
        let out = render_map(&json!(r#"{"a": "{{ n }}"}"#), "values", &cx).unwrap();
        assert_eq!(serde_json::Value::Object(out), json!({"a": "5"}));
        for raw in [
            json!("plain text"),
            json!([1, 2]),
            json!(42),
            json!(true),
            json!(null),
        ] {
            let err = render_map(&raw, "values", &cx).unwrap_err();
            assert!(err.starts_with("`values`"), "{raw}: {err}");
        }
    }

    /// A render failure names where it happened.
    #[test]
    fn render_map_names_the_key_whose_value_failed_to_render() {
        let cx = RenderContext::new();
        let err = render_map(&json!({"a": {"b": "{{ nope.nope }}"}}), "values", &cx).unwrap_err();
        assert!(err.starts_with("`values.a.b`: "), "{err}");
        let err = render_map(&json!({"{{ nope }}": 1}), "values", &cx).unwrap_err();
        assert!(err.starts_with("`values.{{ nope }}`: "), "{err}");
    }
}
