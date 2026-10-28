//! Package #5a, Task A6 (spec D5, Q3): Pebble's `contains`, `..` and the
//! boolean coercion of `and`/`or`.
//!
//! minijinja has no `contains` and no `..`, and its `and`/`or` return an
//! operand (`'a' or false` is `'a'`). [`super::rewrite`] compiles each of
//! Kestra's spellings into a call to a function here, at Pebble's precedence:
//!
//! | Kestra | becomes |
//! |---|---|
//! | `a contains b` | `kestra_contains((a), (b))` |
//! | `a .. b` | `kestra_range(a, b)` |
//! | `a and b`, `a or b` | `kestra_bool(a) and kestra_bool(b)` |
//! | `a equals b` | `a == b` (then `kestra_eq`, `super::compare`) |
//!
//! Each function ports the class Pebble 4.1.2 evaluates (read from the pinned
//! jar with `javap -c -p`):
//!
//! - **`ContainsExpression`**: a null left side is `false` and the right side
//!   is never evaluated; a `Collection` contains the right side by Java's
//!   `equals` (`containsAll` when the right side is a `Collection` too); a
//!   `Map` by `containsKey` (keys only); a `String` by
//!   `String.contains(String.valueOf(right))`; anything else fails with
//!   `Contains operator can only be used on Collections, Maps and arrays.
//!   Actual type was: <class>`.
//! - **`RangeExpression`** builds a call to the function named `range`, which
//!   Kestra registers as `BoundedRangeFunction`; egret's port of it is
//!   `super::functions::range`, called here with the two bounds and no
//!   increment (ascending only: `3 .. 1` is `[]`).
//! - **`AndExpression` / `OrExpression`** short-circuit (minijinja's `and`/`or`
//!   keep that) and pass each operand through
//!   `TypeUtils.compatibleCast(value, Boolean.class)`: a `Boolean` is itself,
//!   a `Number` is true when `doubleValue()` is neither `0` nor `NaN`, a
//!   `String` when it is not empty (`'false'` is true), a null fails with
//!   `null value used in and operator and strict variables is set to true`
//!   (under `or` too: both classes carry the same text), and anything else
//!   fails the `checkcast` with a `ClassCastException`.
//!
//! **What egret cannot carry: Integer vs Long** (spec D2, §2.2 item 10).
//! `[1, 2]` from the context holds `Integer`s and the literal `2` is a `Long`,
//! so `{{ list contains 2 }}` is `false` in Kestra; egret's integers carry no
//! such distinction and answer `true` (`op-contains-boxed-int`, a recorded
//! divergence).

use minijinja::value::{Rest, Value as MjValue, ValueKind};
use minijinja::{Environment, Error as MjError, State};

use super::arith::{java_equals, java_string_value_of};
use super::compare::assert_defined;
use super::filters::{big_number_text, is_big_number, java_class_name, pebble_error};

/// `AndExpression`'s text for a null operand, which `OrExpression` repeats.
pub(super) const NULL_OPERAND: &str =
    "null value used in and operator and strict variables is set to true";

/// `ContainsExpression`'s text for a left side it cannot search.
pub(super) const CONTAINS_REFUSAL: &str =
    "Contains operator can only be used on Collections, Maps and arrays. Actual type was: ";

/// Is this Kestra's `null` (a `none`, or the silent undefined egret spells it
/// with)? A strict undefined has already raised in [`assert_defined`].
fn is_null(value: &MjValue) -> bool {
    value.is_none() || value.is_undefined()
}

/// `a contains b`, `ContainsExpression.evaluate`.
fn kestra_contains(state: &State, left: MjValue, right: MjValue) -> Result<MjValue, MjError> {
    assert_defined(state, &left)?;
    // `if (left == null) return false`, before the right side is evaluated:
    // a missing right side under a null left one is no error in Kestra.
    if is_null(&left) {
        return Ok(MjValue::from(false));
    }
    assert_defined(state, &right)?;
    if is_big_number(&left) {
        return Err(refuse_contains(&left));
    }
    let found = match left.kind() {
        ValueKind::Seq | ValueKind::Iterable => {
            let items: Vec<MjValue> = left.try_iter()?.collect();
            let has = |needle: &MjValue| items.iter().any(|item| java_equals(item, needle));
            if matches!(right.kind(), ValueKind::Seq | ValueKind::Iterable) {
                // `Collection.containsAll`.
                right.try_iter()?.all(|needle| has(&needle))
            } else {
                has(&right)
            }
        }
        // `Map.containsKey`: the keys, never the values.
        ValueKind::Map => left.try_iter()?.any(|key| java_equals(&key, &right)),
        ValueKind::String => {
            let haystack = left.as_str().unwrap_or_default();
            haystack.contains(java_string_value_of(&right).as_str())
        }
        _ => return Err(refuse_contains(&left)),
    };
    Ok(MjValue::from(found))
}

fn refuse_contains(left: &MjValue) -> MjError {
    pebble_error(format!("{CONTAINS_REFUSAL}{}", java_class_name(left)))
}

/// `a .. b`: `RangeExpression` calls Kestra's `range` with the two bounds.
fn kestra_range(state: &State, start: MjValue, end: MjValue) -> Result<MjValue, MjError> {
    assert_defined(state, &start)?;
    assert_defined(state, &end)?;
    super::functions::range(Rest(vec![start, end]))
}

/// One operand of `and`/`or`: `TypeUtils.compatibleCast(value,
/// Boolean.class)` and the `checkcast` after it.
fn kestra_bool(state: &State, value: MjValue) -> Result<bool, MjError> {
    assert_defined(state, &value)?;
    if is_null(&value) {
        return Err(pebble_error(NULL_OPERAND));
    }
    if is_big_number(&value) {
        // `Number.doubleValue()`.
        let double = big_number_text(&value)
            .and_then(|text| text.parse::<f64>().ok())
            .unwrap_or(0.0);
        return Ok(double != 0.0);
    }
    match value.kind() {
        ValueKind::Bool => Ok(value.is_true()),
        ValueKind::Number => {
            if value.is_integer() {
                // An `i64` or wider integer: non-zero is all `doubleValue()`
                // can tell apart here.
                return Ok(value.is_true());
            }
            let double = f64::try_from(value).unwrap_or(f64::NAN);
            Ok(!double.is_nan() && double != 0.0)
        }
        ValueKind::String => Ok(!value.as_str().unwrap_or_default().is_empty()),
        _ => {
            let class = java_class_name(&value);
            Err(pebble_error(format!(
                "class {class} cannot be cast to class java.lang.Boolean ({class} and \
                 java.lang.Boolean are in module java.base of loader 'bootstrap')"
            )))
        }
    }
}

/// Registers the three functions.
pub(super) fn register(env: &mut Environment<'static>) {
    env.add_function("kestra_contains", kestra_contains);
    env.add_function("kestra_range", kestra_range);
    env.add_function("kestra_bool", kestra_bool);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runners::env::{render_str, RenderContext};
    use serde_json::json;

    fn render_in(template: &str, ctx: serde_json::Value) -> Result<String, String> {
        let cx = RenderContext::from_json_str(&ctx.to_string()).expect("a JSON object");
        render_str(template, &cx).map_err(|e| e.to_string())
    }

    fn ok(template: &str, ctx: serde_json::Value) -> String {
        render_in(template, ctx).unwrap_or_else(|e| panic!("{template}: {e}"))
    }

    #[test]
    fn contains_searches_what_pebble_searches() {
        for (template, ctx, want) in [
            // A list: an element, by Java's `equals`.
            ("{{ [1, 2] contains 2 }}", json!({}), "true"),
            ("{{ [1, 2] contains 3 }}", json!({}), "false"),
            ("{{ [1, 2] contains 2.0 }}", json!({}), "false"),
            ("{{ [1, 2] contains '2' }}", json!({}), "false"),
            ("{{ l contains n }}", json!({"l": [1, 2], "n": 2}), "true"),
            ("{{ ['a', 'b'] contains 'a' }}", json!({}), "true"),
            ("{{ l contains null }}", json!({"l": [1, null]}), "true"),
            // A list on the right: `containsAll`.
            ("{{ [1, 2, 3] contains [1, 2] }}", json!({}), "true"),
            ("{{ [1, 2, 3] contains [1, 4] }}", json!({}), "false"),
            ("{{ [1, 2] contains [] }}", json!({}), "true"),
            ("{{ [[1], 2] contains [[1]] }}", json!({}), "true"),
            // A map: its keys only.
            ("{{ m contains 'a' }}", json!({"m": {"a": 1}}), "true"),
            ("{{ m contains 1 }}", json!({"m": {"a": 1}}), "false"),
            // A string: `String.valueOf(right)` as a substring.
            ("{{ 'abc' contains 'b' }}", json!({}), "true"),
            ("{{ 'abc' contains 'z' }}", json!({}), "false"),
            ("{{ 'a1' contains 1 }}", json!({}), "true"),
            ("{{ 'anull' contains null }}", json!({}), "true"),
            // A null left side is false, and the right side is not needed.
            ("{{ n contains 1 }}", json!({"n": null}), "false"),
            ("{{ n contains missing }}", json!({"n": null}), "false"),
            // `contains` binds looser than `==` and tighter than `and`.
            ("{{ [true] contains 1 == 1 }}", json!({}), "true"),
            ("{{ [1] contains 1 and true }}", json!({}), "true"),
            (
                "{% if l contains 'x' %}y{% else %}n{% endif %}",
                json!({"l": ["x"]}),
                "y",
            ),
        ] {
            assert_eq!(ok(template, ctx), want, "{template}");
        }
    }

    /// `operator-contains-number-left` is error-class: it holds the refusal to
    /// Kestra's own text, not to minijinja's parse error it used to be.
    #[test]
    fn contains_on_what_pebble_cannot_search_fails_with_kestras_text() {
        for (template, ctx, class) in [
            ("{{ n contains 1 }}", json!({"n": 5}), "java.lang.Integer"),
            (
                "{{ b contains 1 }}",
                json!({"b": true}),
                "java.lang.Boolean",
            ),
            ("{{ 1.5 contains 1 }}", json!({}), "java.lang.Double"),
        ] {
            let error = render_in(template, ctx).unwrap_err();
            assert!(
                error.contains(&format!("{CONTAINS_REFUSAL}{class}")),
                "{template}: {error}"
            );
        }
        let error = render_in("{{ missing contains 1 }}", json!({})).unwrap_err();
        assert!(error.contains("undefined"), "{error}");
        let error = render_in("{{ [1] contains missing }}", json!({})).unwrap_err();
        assert!(error.contains("undefined"), "{error}");
    }

    #[test]
    fn a_range_is_kestras_range_function() {
        for (template, ctx, want) in [
            ("{{ 1 .. 3 }}", json!({}), "[1,2,3]"),
            ("{{ 1..3 }}", json!({}), "[1,2,3]"),
            ("{{ 3 .. 1 }}", json!({}), "[]"),
            ("{{ 2 .. 2 }}", json!({}), "[2]"),
            ("{{ -1 .. 1 }}", json!({}), "[-1,0,1]"),
            ("{{ 'a' .. 'c' }}", json!({}), "[\"a\",\"b\",\"c\"]"),
            ("{{ 'c' .. 'a' }}", json!({}), "[]"),
            ("{{ a .. b }}", json!({"a": 1, "b": 3}), "[1,2,3]"),
            ("{{ o.a .. o.b }}", json!({"o": {"a": 1, "b": 2}}), "[1,2]"),
            ("{{ l[0] .. l[1] }}", json!({"l": [1, 2]}), "[1,2]"),
            ("{{ 1.5 .. 3 }}", json!({}), "[1,2,3]"),
            // Binds tighter than `+`, `~` and a filter.
            ("{{ 1 .. 3 | length }}", json!({}), "3"),
            ("{{ 'x' ~ 1 .. 2 }}", json!({}), "x[1, 2]"),
            ("{{ (1 .. 3) | length }}", json!({}), "3"),
            ("{{ [1 .. 2] | length }}", json!({}), "1"),
            (
                "{% for i in 1 .. 3 %}{{ i }};{% endfor %}",
                json!({}),
                "1;2;3;",
            ),
            ("{% for i in 3 .. 1 %}{{ i }};{% endfor %}", json!({}), ""),
            ("{% set r = 1 .. 2 %}{{ r }}", json!({}), "[1,2]"),
            ("{{ 2 isIn 1 .. 3 }}", json!({}), "true"),
            ("{{ 1 .. 3 contains 2 }}", json!({}), "true"),
            // A `.` that is not `..` is untouched.
            ("{{ 1.5 }}", json!({}), "1.5"),
            ("{{ '1..3' }}", json!({}), "1..3"),
        ] {
            assert_eq!(ok(template, ctx), want, "{template}");
        }
    }

    #[test]
    fn a_range_kestra_refuses_is_refused_here_too() {
        for template in ["{{ 'ab' .. 'c' }}", "{{ 1 .. 'c' }}", "{{ n .. 2 }}"] {
            let error = render_in(template, json!({"n": null})).unwrap_err();
            assert!(
                error.contains(
                    "Arguments of range function must be of type Number or String with a length of 1"
                ),
                "{template}: {error}"
            );
        }
        let error = render_in("{{ 1 .. missing }}", json!({})).unwrap_err();
        assert!(error.contains("undefined"), "{error}");
    }

    #[test]
    fn and_and_or_coerce_each_operand_to_a_boolean() {
        for (template, ctx, want) in [
            ("{{ true and false }}", json!({}), "false"),
            ("{{ 1 and true }}", json!({}), "true"),
            ("{{ 0 and true }}", json!({}), "false"),
            ("{{ 2 and true }}", json!({}), "true"),
            ("{{ 0.0 or false }}", json!({}), "false"),
            ("{{ 0.5 or false }}", json!({}), "true"),
            ("{{ '' or false }}", json!({}), "false"),
            ("{{ 'false' or false }}", json!({}), "true"),
            ("{{ true and 'x' }}", json!({}), "true"),
            ("{{ false or 0 }}", json!({}), "false"),
            (
                "{{ a and b and c }}",
                json!({"a": 1, "b": "x", "c": true}),
                "true",
            ),
            (
                "{{ a or b or c }}",
                json!({"a": 0, "b": "", "c": 0}),
                "false",
            ),
            // Short circuit: the right side is never evaluated.
            ("{{ false and missing.y }}", json!({}), "false"),
            ("{{ true or missing.y }}", json!({}), "true"),
            ("{{ false and l }}", json!({"l": [1]}), "false"),
            ("{% if 1 and 'x' %}y{% else %}n{% endif %}", json!({}), "y"),
            ("{% set b = 0 or '' %}{{ b }}", json!({}), "false"),
            ("{{ (1 and 1) ? 'y' : 'n' }}", json!({}), "y"),
        ] {
            assert_eq!(ok(template, ctx), want, "{template}");
        }
    }

    /// `logic-and-returns-boolean` (`{{ (true and 'x') ~ '|' }}`, Kestra
    /// `true|`) stays baselined for a cause outside this module: the result
    /// IS the boolean `true` now, but minijinja's `~` writes any boolean as
    /// `True`, a literal `true` included -- `~` is not compiled to a Pebble
    /// port (`super::cmp_rewrite::compile_arithmetic` leaves it alone).
    #[test]
    fn the_boolean_reaches_tilde_and_tilde_spells_it_in_python() {
        assert_eq!(ok("{{ (true and 'x') == true }}", json!({})), "true");
        assert_eq!(
            ok("{{ (true and 'x') | className }}", json!({})),
            "java.lang.Boolean"
        );
        assert_eq!(ok("{{ (true and 'x') ~ '|' }}", json!({})), "True|");
        assert_eq!(ok("{{ true ~ '|' }}", json!({})), "True|");
    }

    /// `logic-and-list-left`, `logic-and-map-left`, `logic-or-null-context`
    /// and the two `null-literal-in-an-*-operand` entries are error-class:
    /// they fail here for Kestra's reason, with Kestra's text.
    #[test]
    fn an_operand_kestra_cannot_cast_fails_with_kestras_text() {
        for (template, ctx, text) in [
            (
                "{{ l and true }}",
                json!({"l": [1]}),
                "class java.util.ArrayList cannot be cast to class java.lang.Boolean",
            ),
            (
                "{{ m and true }}",
                json!({"m": {"a": 1}}),
                "class java.util.LinkedHashMap cannot be cast to class java.lang.Boolean",
            ),
            ("{{ n or false }}", json!({"n": null}), NULL_OPERAND),
            // `null-literal-in-an-and-operand`, `null-literal-in-an-or-operand`.
            ("<{{ null and true }}>", json!({}), NULL_OPERAND),
            ("<{{ null or 'z' }}>", json!({}), NULL_OPERAND),
            ("{{ true and n }}", json!({"n": null}), NULL_OPERAND),
            (
                "{% if n and true %}y{% endif %}",
                json!({"n": null}),
                NULL_OPERAND,
            ),
        ] {
            let error = render_in(template, ctx).unwrap_err();
            assert!(error.contains(text), "{template}: {error}");
        }
        let error = render_in("{{ missing and true }}", json!({})).unwrap_err();
        assert!(error.contains("undefined"), "{error}");
    }

    #[test]
    fn equals_is_pebbles_double_equals() {
        for (template, ctx, want) in [
            ("{{ a equals b }}", json!({"a": 1, "b": 1}), "true"),
            ("{{ 1 equals 1.0 }}", json!({}), "true"),
            ("{{ 1 equals '1' }}", json!({}), "false"),
            ("{{ a equals 'x' }}", json!({"a": "y"}), "false"),
            ("{{ 1 equals 1 and 2 equals 2 }}", json!({}), "true"),
            ("{{ 1 + 1 equals 2 }}", json!({}), "true"),
            ("{{ x.equals }}", json!({"x": {"equals": "attr"}}), "attr"),
            (
                "{% if a equals 'v' %}y{% else %}n{% endif %}",
                json!({"a": "v"}),
                "y",
            ),
        ] {
            assert_eq!(ok(template, ctx), want, "{template}");
        }
    }
}
