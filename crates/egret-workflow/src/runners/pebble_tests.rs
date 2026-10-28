//! Package #5b, Task B2 (spec D8): the engine's `is` surface.
//!
//! Kestra's Pebble engine carries eight tests: Pebble's own `defined`,
//! `empty`, `even`, `iterable`, `map`, `null` and `odd`, and Kestra's `json`
//! (`Extension.getTests()`). `defined` is [`super::env`]'s (W2b); this module
//! registers the other seven, three of them overriding a minijinja default of
//! the same name that answered differently (`even`, `odd`, `iterable`).
//!
//! Each one ports the class the engine evaluates, read from the pinned jar
//! with `javap -c -p` (Pebble 4.1.2) and from Kestra's source (`JsonTest`):
//!
//! | test | Pebble/Kestra | a value it refuses |
//! |---|---|---|
//! | `null` | `input == null` | none |
//! | `empty` | null; a `String` whose `trim()` is `""`; an empty `Collection` or `Map` | none |
//! | `map` | `instanceof Map` | none |
//! | `iterable` | `instanceof Iterable` or `Object[]` (a String is not) | none |
//! | `even`, `odd` | `Integer.intValue()` or `((Long) input).longValue()`, then `% 2` | null; anything not an `Integer`/`Long` (`ClassCastException`) |
//! | `json` | `JacksonMapper.ofJson().readTree((String) input)` succeeds | null (`argument "content" is null`); a non-String (`ClassCastException`) |
//!
//! **Every test ignores its arguments.** Each class's `getArgumentNames()` is
//! `null`, so Pebble passes positional arguments through unread: measured,
//! `{{ 'a' is json(1) }}` and `{{ 'a' is map(1) }}` are `false`, not errors.
//!
//! **A missing variable fails, as Kestra's strict variables do.** Kestra
//! renders under `strictVariables(true)`, so `{{ x is null }}` with no `x`
//! is a `RootAttributeNotFoundException` before the test runs
//! (`test-null-missing-root`, `test-empty-missing-root`). minijinja hands a
//! test the undefined value; [`assert_defined`] raises the VM's own
//! `UndefinedError` for it, and the silent undefined of a missing `else`
//! (no Kestra spelling reaches one) is Java's `null`.
//!
//! # `null` is registered as `null`, not as `none`
//!
//! D8 reads "reached through the `none` spelling the rewriter produces; the
//! test is registered as `none`". Measured before building it, that would
//! over-accept: Kestra refuses `{{ x is none }}`, `{{ x is NONE }}` and
//! `{{ x is NULL }}` (`Test [none] does not exist.`), because the null
//! literal's four spellings are a *literal*'s and a test name is a bare
//! identifier. The rewriter therefore leaves a word in test-name position
//! alone (`rewrite`'s `follows_a_test_operator`), and the test is bound under
//! its own name. `none` stays removed (`env::MINIJINJA_ONLY_TESTS`).
//!
//! # `json`: Jackson's `readTree`, not `serde_json`
//!
//! `readTree(String)` reads ONE value and stops (`FAIL_ON_TRAILING_TOKENS` is
//! off), and an empty or whitespace-only string is a `MissingNode`, not an
//! error. [`jackson_reads_a_tree`] ports what the parser checks on the way to
//! that first value; every rule below was measured through the harness
//! (jackson-core 2.22.1, Kestra's default `StreamReadConstraints`):
//!
//! - whitespace is exactly space, tab, CR and LF (a no-break space, a form
//!   feed, a vertical tab, a BOM or a NUL before the value fails);
//! - a root-level **number** must be followed by that whitespace or the end
//!   (`1 x` is `true`, `1x`, `1,`, `1]` and `1/` are `false`); a string or a
//!   container needs nothing after it (`"s"x`, `[1]x`, `{} {` are `true`);
//! - `true`, `false` and `null` fail when the next character is at or above
//!   `'0'`, not `]` or `}`, and a Java identifier part (`true1`, `true_`,
//!   `nullx`); `true,`, `true$` and `true(` are `true`;
//! - nesting deeper than 1 000 fails (`maxNestingDepth`), as does an integer
//!   of more than 1 000 digits (`maxNumberLength`); a fraction of 1 000
//!   digits does not;
//! - no comments, no single quotes, no `NaN`/`Infinity`, no leading `+` or
//!   `0`, no trailing comma, no unescaped control character inside a string;
//!   a lone surrogate escape (`"\ud800"`) is accepted.
//!
//! Only `Character.isJavaIdentifierPart` is approximated: by
//! `char::is_alphanumeric`, `_`, and U+007F..U+009F (the identifier-ignorable
//! controls at or above `'0'`). A currency symbol other than `$` or a
//! combining mark straight after a literal would be refused in Kestra and
//! accepted here; no flow writes one.

use minijinja::value::{Rest, Value as MjValue, ValueKind};
use minijinja::{Environment, Error as MjError, State};

use super::compare::assert_defined;
use super::filters::{is_java_null, java_class_name, pebble_error};

/// `EvenTest`'s text for a null input (a `PebbleException`).
pub(super) const EVEN_OF_NULL: &str = "Can not pass null value to \"even\" test.";

/// `OddTest`'s text for a null input (an `IllegalArgumentException`).
pub(super) const ODD_OF_NULL: &str = "Can not pass null value to \"odd\" test.";

/// `JsonTest`'s text for a null input: Jackson's `_assertNotNull("content")`.
pub(super) const JSON_OF_NULL: &str = "argument \"content\" is null";

/// Jackson's `StreamReadConstraints.DEFAULT_MAX_DEPTH`.
const MAX_NESTING_DEPTH: usize = 1000;

/// Jackson's `StreamReadConstraints.DEFAULT_MAX_NUM_LEN`, which `readTree`
/// enforces on an integer (measured: 1 000 digits pass, 1 001 fail).
const MAX_INTEGER_DIGITS: usize = 1000;

/// The value a test sees: a strict undefined has raised, and a silent one is
/// Java's `null`.
fn input(state: &State, value: MjValue) -> Result<MjValue, MjError> {
    assert_defined(state, &value)?;
    Ok(if value.is_undefined() {
        MjValue::from(())
    } else {
        value
    })
}

/// `NullTest`: `input == null`.
fn null(state: &State, value: MjValue, _args: Rest<MjValue>) -> Result<bool, MjError> {
    Ok(is_java_null(&input(state, value)?))
}

/// `EmptyTest`: null; a String whose `trim()` is empty; an empty Collection or
/// Map. Anything else (`0`, `false`) is not empty.
fn empty(state: &State, value: MjValue, _args: Rest<MjValue>) -> Result<bool, MjError> {
    let value = input(state, value)?;
    if is_java_null(&value) {
        return Ok(true);
    }
    Ok(match value.kind() {
        // Java's `trim()` strips every char at or below U+0020, so a no-break
        // space (U+00A0) is not empty.
        ValueKind::String => value
            .as_str()
            .is_some_and(|text| text.trim_matches(|c: char| c <= ' ').is_empty()),
        ValueKind::Seq | ValueKind::Iterable | ValueKind::Map => {
            value.len().map(|n| n == 0).unwrap_or_else(|| {
                value
                    .try_iter()
                    .map(|mut items| items.next().is_none())
                    .unwrap_or(false)
            })
        }
        _ => false,
    })
}

/// `MapTest`: `instanceof Map`.
fn map(state: &State, value: MjValue, _args: Rest<MjValue>) -> Result<bool, MjError> {
    Ok(input(state, value)?.kind() == ValueKind::Map)
}

/// `IterableTest`: `instanceof Iterable` or an `Object[]`. A String is
/// neither, nor is a Map.
fn iterable(state: &State, value: MjValue, _args: Rest<MjValue>) -> Result<bool, MjError> {
    Ok(matches!(
        input(state, value)?.kind(),
        ValueKind::Seq | ValueKind::Iterable
    ))
}

/// `EvenTest.apply`'s number: an `Integer`'s `intValue()`, otherwise a
/// `(Long)` cast. egret's integers carry no Integer/Long box, and both take
/// the same branch for every value in `long` range; anything else (a Double,
/// a `BigInteger`, a `BigDecimal`, a String) fails the cast.
fn pebble_long(value: &MjValue, null_text: &str) -> Result<i64, MjError> {
    if is_java_null(value) {
        return Err(pebble_error(null_text));
    }
    if value.kind() == ValueKind::Number && value.is_integer() {
        if let Ok(n) = i64::try_from(value.clone()) {
            return Ok(n);
        }
    }
    Err(pebble_error(format!(
        "class {} cannot be cast to class java.lang.Long",
        java_class_name(value)
    )))
}

/// `EvenTest`.
fn even(state: &State, value: MjValue, _args: Rest<MjValue>) -> Result<bool, MjError> {
    Ok(pebble_long(&input(state, value)?, EVEN_OF_NULL)? % 2 == 0)
}

/// `OddTest`: its own null check, then `!EvenTest.apply(..)`.
fn odd(state: &State, value: MjValue, _args: Rest<MjValue>) -> Result<bool, MjError> {
    Ok(pebble_long(&input(state, value)?, ODD_OF_NULL)? % 2 != 0)
}

/// Kestra's `JsonTest`: `readTree((String) input)` succeeds.
fn json(state: &State, value: MjValue, _args: Rest<MjValue>) -> Result<bool, MjError> {
    let value = input(state, value)?;
    if is_java_null(&value) {
        return Err(pebble_error(JSON_OF_NULL));
    }
    match value.as_str() {
        Some(text) if value.kind() == ValueKind::String => Ok(jackson_reads_a_tree(text)),
        _ => Err(pebble_error(format!(
            "class {} cannot be cast to class java.lang.String",
            java_class_name(&value)
        ))),
    }
}

/// Does `ObjectMapper.readTree(text)` return rather than throw? See the
/// module doc for the measured rules.
pub(super) fn jackson_reads_a_tree(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let mut at = skip_ws(&chars, 0);
    if at == chars.len() {
        // `MissingNode`: an empty document is not an error to `readTree`.
        return true;
    }
    let root_is_number = chars[at] == '-' || chars[at].is_ascii_digit();
    match value(&chars, at) {
        Some(end) => at = end,
        None => return false,
    }
    if root_is_number {
        // `_verifyRootSpace`: a root-level number must end at whitespace.
        return at == chars.len() || matches!(chars[at], ' ' | '\t' | '\r' | '\n');
    }
    true
}

fn skip_ws(chars: &[char], mut at: usize) -> usize {
    while at < chars.len() && matches!(chars[at], ' ' | '\t' | '\r' | '\n') {
        at += 1;
    }
    at
}

/// The container a value is being read inside.
enum Container {
    Array,
    Object,
}

/// Parses one JSON value starting at `start`, returning the index just past
/// it, or `None` where Jackson throws. Iterative, so the 1 000-deep limit
/// costs no stack.
fn value(chars: &[char], start: usize) -> Option<usize> {
    let mut stack: Vec<Container> = Vec::new();
    let mut at = start;
    'value: loop {
        at = skip_ws(chars, at);
        match *chars.get(at)? {
            open @ ('[' | '{') => {
                if stack.len() == MAX_NESTING_DEPTH {
                    return None;
                }
                let next = skip_ws(chars, at + 1);
                if open == '[' {
                    if chars.get(next) == Some(&']') {
                        at = next + 1;
                    } else {
                        stack.push(Container::Array);
                        at = next;
                        continue 'value;
                    }
                } else if chars.get(next) == Some(&'}') {
                    at = next + 1;
                } else {
                    stack.push(Container::Object);
                    at = member_key(chars, next)?;
                    continue 'value;
                }
            }
            '"' => at = string(chars, at)?,
            '-' | '0'..='9' => at = number(chars, at)?,
            't' => at = literal(chars, at, "true")?,
            'f' => at = literal(chars, at, "false")?,
            'n' => at = literal(chars, at, "null")?,
            _ => return None,
        }
        // A value ended at `at`: read separators and closes until the next
        // value starts or the root value ends.
        loop {
            let Some(top) = stack.last() else {
                return Some(at);
            };
            at = skip_ws(chars, at);
            match (top, *chars.get(at)?) {
                (Container::Array, ',') => {
                    at += 1;
                    continue 'value;
                }
                (Container::Object, ',') => {
                    at = member_key(chars, skip_ws(chars, at + 1))?;
                    continue 'value;
                }
                (Container::Array, ']') | (Container::Object, '}') => {
                    at += 1;
                    stack.pop();
                }
                _ => return None,
            }
        }
    }
}

/// Reads `"key"` then `:` at `at` (whitespace already skipped), returning the
/// index of the member's value (whitespace before it is skipped by the
/// caller). A trailing comma (`{"a":1,}`) and an unquoted key fail here.
fn member_key(chars: &[char], at: usize) -> Option<usize> {
    if chars.get(at) != Some(&'"') {
        return None;
    }
    let after = skip_ws(chars, string(chars, at)?);
    (chars.get(after) == Some(&':')).then_some(after + 1)
}

/// A JSON string at `at` (the opening quote); the index past its close.
fn string(chars: &[char], mut at: usize) -> Option<usize> {
    at += 1;
    loop {
        match *chars.get(at)? {
            '"' => return Some(at + 1),
            '\\' => {
                match *chars.get(at + 1)? {
                    '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't' => at += 2,
                    'u' => {
                        // Four hex digits; a lone surrogate is accepted.
                        let hex = chars.get(at + 2..at + 6)?;
                        if !hex.iter().all(char::is_ascii_hexdigit) {
                            return None;
                        }
                        at += 6;
                    }
                    _ => return None,
                }
            }
            c if c < ' ' => return None,
            _ => at += 1,
        }
    }
}

/// A JSON number at `at`; the index past it. Leading `+`, a leading zero
/// before a digit, a bare `-`, `1.` and `1e` fail; an integer of more than
/// [`MAX_INTEGER_DIGITS`] digits fails.
fn number(chars: &[char], mut at: usize) -> Option<usize> {
    if chars.get(at) == Some(&'-') {
        at += 1;
    }
    let int_start = at;
    while chars.get(at).is_some_and(char::is_ascii_digit) {
        at += 1;
    }
    let int_digits = at - int_start;
    if int_digits == 0 || (int_digits > 1 && chars[int_start] == '0') {
        return None;
    }
    let mut integral = true;
    if chars.get(at) == Some(&'.') {
        integral = false;
        at += 1;
        let start = at;
        while chars.get(at).is_some_and(char::is_ascii_digit) {
            at += 1;
        }
        if at == start {
            return None;
        }
    }
    if matches!(chars.get(at), Some('e' | 'E')) {
        integral = false;
        at += 1;
        if matches!(chars.get(at), Some('+' | '-')) {
            at += 1;
        }
        let start = at;
        while chars.get(at).is_some_and(char::is_ascii_digit) {
            at += 1;
        }
        if at == start {
            return None;
        }
    }
    if integral && int_digits > MAX_INTEGER_DIGITS {
        return None;
    }
    Some(at)
}

/// `true`/`false`/`null` at `at`; the index past it. Jackson's
/// `_matchToken`/`_checkMatchEnd`: a following character at or above `'0'`
/// that is not `]` or `}` and is a Java identifier part makes the token
/// unrecognised.
fn literal(chars: &[char], at: usize, word: &str) -> Option<usize> {
    let end = at + word.chars().count();
    if chars.get(at..end)?.iter().copied().ne(word.chars()) {
        return None;
    }
    match chars.get(end) {
        Some(&c) if c >= '0' && c != ']' && c != '}' && is_java_identifier_part(c) => None,
        _ => Some(end),
    }
}

/// `Character.isJavaIdentifierPart`, approximated (module doc).
fn is_java_identifier_part(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || ('\u{7f}'..='\u{9f}').contains(&c)
}

pub(super) fn register(env: &mut Environment<'static>) {
    env.add_test("null", null);
    env.add_test("empty", empty);
    env.add_test("map", map);
    env.add_test("iterable", iterable);
    env.add_test("even", even);
    env.add_test("odd", odd);
    env.add_test("json", json);
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

    /// Every expected value here was measured through the oracle harness
    /// (`io.kestra:core:2.0.0-rc13`, Pebble 4.1.2), most of them as corpus
    /// entries (`test-*`); the rest from B2's probe run, recorded in the task
    /// report.
    #[test]
    fn the_seven_tests_answer_as_pebble() {
        for (template, ctx, want) in [
            // null
            ("{{ x is null }}", json!({"x": null}), "true"),
            ("{{ x is null }}", json!({"x": 0}), "false"),
            ("{{ x is null }}", json!({"x": ""}), "false"),
            ("{{ x is not null }}", json!({"x": 1}), "true"),
            ("{{ null is null }}", json!({}), "true"),
            (
                "{% if x is null %}y{% else %}n{% endif %}",
                json!({"x": null}),
                "y",
            ),
            (
                "{{ a ?? b is not null }}",
                json!({"a": "A", "b": "B"}),
                "true",
            ),
            // empty
            ("{{ x is empty }}", json!({"x": ""}), "true"),
            ("{{ x is empty }}", json!({"x": " \t\n"}), "true"),
            ("{{ x is empty }}", json!({"x": "\u{a0}"}), "false"),
            ("{{ x is empty }}", json!({"x": " \u{2003} "}), "false"),
            ("{{ x is empty }}", json!({"x": []}), "true"),
            ("{{ x is empty }}", json!({"x": {}}), "true"),
            ("{{ x is empty }}", json!({"x": null}), "true"),
            ("{{ null is empty }}", json!({}), "true"),
            ("{{ x is empty }}", json!({"x": 0}), "false"),
            ("{{ x is empty }}", json!({"x": false}), "false"),
            ("{{ 1.5 is empty }}", json!({}), "false"),
            ("{{ x is empty }}", json!({"x": [1]}), "false"),
            ("{{ x is empty }}", json!({"x": {"a": 1}}), "false"),
            ("{{ [] is empty }}", json!({}), "true"),
            ("{{ x is not empty }}", json!({"x": " "}), "false"),
            // map
            ("{{ x is map }}", json!({"x": {}}), "true"),
            ("{{ {'a': 1} is map }}", json!({}), "true"),
            ("{{ x is map }}", json!({"x": []}), "false"),
            ("{{ x is map }}", json!({"x": "a"}), "false"),
            ("{{ x is map }}", json!({"x": null}), "false"),
            ("{{ null is map }}", json!({}), "false"),
            ("{{ 'a' is map(1) }}", json!({}), "false"),
            // iterable
            ("{{ [1] is iterable }}", json!({}), "true"),
            ("{{ 'abc' is iterable }}", json!({}), "false"),
            ("{{ s is iterable }}", json!({"s": ""}), "false"),
            ("{{ m is iterable }}", json!({"m": {"a": 1}}), "false"),
            ("{{ n is iterable }}", json!({"n": null}), "false"),
            // even / odd
            ("{{ n is even }}", json!({"n": 2}), "true"),
            ("{{ n is odd }}", json!({"n": 3}), "true"),
            ("{{ -3 is odd }}", json!({}), "true"),
            ("{{ n is even }}", json!({"n": 9_000_000_000_i64}), "true"),
            // json
            ("{{ x is json }}", json!({"x": "{\"key\":1}"}), "true"),
            ("{{ x is json }}", json!({"x": "plain"}), "false"),
            ("{{ x is not json }}", json!({"x": "plain"}), "true"),
            ("{{ x is json }}", json!({"x": ""}), "true"),
            ("{{ 'a' is json(1) }}", json!({}), "false"),
        ] {
            assert_eq!(ok(template, ctx.clone()), want, "{template} {ctx}");
        }
    }

    /// The names Kestra does not have stay refused: the null literal's other
    /// spellings are not test names (measured: `Test [none] does not
    /// exist.`, likewise `NONE` and `NULL`).
    #[test]
    fn only_null_names_the_null_test() {
        for template in [
            "{{ x is none }}",
            "{{ x is not none }}",
            "{{ x is NONE }}",
            "{{ x is NULL }}",
        ] {
            let error = render_in(template, json!({"x": null})).unwrap_err();
            assert!(error.contains("unknown test"), "{template}: {error}");
        }
    }

    /// `readTree`'s acceptance, one row per measured rule (module doc).
    #[test]
    fn json_is_jacksons_read_tree() {
        let deep = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
        for (text, want) in [
            ("1", true),
            ("[1, 2]", true),
            ("\"s\"", true),
            ("true", true),
            ("null", true),
            ("", true),
            ("   ", true),
            ("  1  ", true),
            ("\r1\r", true),
            ("1 x", true),
            ("0 1", true),
            ("1 /* x */", true),
            ("{} {", true),
            ("\"s\"x", true),
            ("[1]x", true),
            ("true,", true),
            ("true]", true),
            ("null,", true),
            ("true$", true),
            ("true(", true),
            ("-0", true),
            ("0.5", true),
            ("1e5", true),
            ("1E+2", true),
            ("{\"a\":1,\"a\":2}", true),
            ("\"\\ud800\"", true),
            ("\"\u{7f}\"", true),
            ("plain", false),
            ("{", false),
            ("x1", false),
            ("1x", false),
            ("1,", false),
            ("1]", false),
            ("1}", false),
            ("1\"", false),
            ("1/", false),
            ("1#", false),
            ("truex", false),
            ("true1", false),
            ("true_", false),
            ("nullx", false),
            ("\u{a0}1", false),
            ("1\u{a0}", false),
            ("\u{c}1", false),
            ("\u{b}1", false),
            ("\u{0}1", false),
            ("1\u{0}", false),
            ("\u{feff}1", false),
            ("\"a\u{1}\"", false),
            ("\"a\tb\"", false),
            ("\"\\x\"", false),
            ("\"\\u12G4\"", false),
            ("\"abc", false),
            ("1.", false),
            ("1e", false),
            ("+1", false),
            ("-", false),
            ("01", false),
            ("-01", false),
            (".5", false),
            ("0x1", false),
            ("'a'", false),
            ("[1,]", false),
            ("{1:2}", false),
            ("NaN", false),
            ("Infinity", false),
            ("-Infinity", false),
            ("# c\n1", false),
            ("/* c */ 1", false),
            ("{'a':1}", false),
        ] {
            assert_eq!(jackson_reads_a_tree(text), want, "{text:?}");
        }
        assert!(jackson_reads_a_tree(&deep(129)));
        assert!(jackson_reads_a_tree(&deep(1000)));
        assert!(!jackson_reads_a_tree(&deep(1001)));
        assert!(jackson_reads_a_tree(&"1".repeat(1000)));
        assert!(!jackson_reads_a_tree(&"1".repeat(1001)));
        assert!(jackson_reads_a_tree(&format!("1.{}", "1".repeat(1000))));
        // Nested members and separators.
        assert!(jackson_reads_a_tree(
            "{\"a\": [1, {\"b\": null}], \"c\": \"d\"}"
        ));
        assert!(!jackson_reads_a_tree("{\"a\" 1}"));
        assert!(!jackson_reads_a_tree("[1 2]"));
        assert!(!jackson_reads_a_tree("{\"a\":1 \"b\":2}"));
    }

    /// `even`/`odd` refuse what Pebble's `(Long)` cast refuses, and a null,
    /// with Kestra's text (`test-even`, `test-odd`, `test-even-numeric-string`,
    /// `test-odd-numeric-string`, `test-even-string-literal`,
    /// `test-even-double`).
    #[test]
    fn even_and_odd_fail_with_pebbles_text() {
        for (template, ctx, class) in [
            ("{{ s is even }}", json!({"s": "x"}), "java.lang.String"),
            ("{{ s is odd }}", json!({"s": "x"}), "java.lang.String"),
            ("{{ s is even }}", json!({"s": "2"}), "java.lang.String"),
            ("{{ s is odd }}", json!({"s": "3"}), "java.lang.String"),
            ("{{ 'x' is even }}", json!({}), "java.lang.String"),
            ("{{ n is even }}", json!({"n": 2.0}), "java.lang.Double"),
            ("{{ 2.0 is even }}", json!({}), "java.lang.Double"),
            ("{{ true is even }}", json!({}), "java.lang.Boolean"),
            (
                "{{ n is even }}",
                json!({"n": 12_345_678_901_234_567_890_u64}),
                "java.math.BigInteger",
            ),
        ] {
            let error = render_in(template, ctx).unwrap_err();
            assert!(
                error.contains(&format!(
                    "class {class} cannot be cast to class java.lang.Long"
                )),
                "{template}: {error}"
            );
        }
        let error = render_in("{{ x is even }}", json!({"x": null})).unwrap_err();
        assert!(error.contains(EVEN_OF_NULL), "{error}");
        let error = render_in("{{ x is odd }}", json!({"x": null})).unwrap_err();
        assert!(error.contains(ODD_OF_NULL), "{error}");
    }
}
