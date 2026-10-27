// kestra: core/src/main/java/io/kestra/core/runners/FlowInputOutput.java (resolveInputs, parseType)
// kestra: core/src/main/java/io/kestra/core/models/flows/input/*.java (validate)
// kestra: core/src/main/java/io/kestra/core/utils/TypeConverter.java
// kestra: core/src/main/java/io/kestra/core/exceptions/InputOutputValidationException.java
// jdk: java.time.{Duration,Instant,LocalDate,LocalTime}#parse/#toString, java.lang.Float#toString
//! The start-time input check (package #3, spec D1-D5, D9, D10): the flow's
//! declared `inputs:` against the posted object, giving the **coerced** object
//! that is stored (D2) or the **first** error in declared order (D1).
//!
//! Pure: no database, no renderer, no clock. It reads nothing but its two
//! arguments, so the start path, the subflow child and (B1) the validate
//! endpoint can share it and can never disagree.
//!
//! # Per input, in declared order (`FlowInputOutput.resolveInputs`)
//!
//! 1. An explicit `null`, or `""` for any type but STRING, SELECT, EMAIL and
//!    SECRET (D5, `isTextType`), is **absent**.
//! 2. Absent with a `defaults`: a literal default (no `{{`/`{%`) is parsed and
//!    validated like a posted value, and stays absent in the result -- the
//!    render still fills it (D2). A templated default is not checked here.
//! 3. Absent with no default: `Missing required input:<id>` when `required`
//!    (default true), otherwise absent (Kestra keeps a `null`; egret leaves the
//!    key out, which renders the same).
//! 4. Parse, then validate. A failure is
//!    ``Invalid value for input `<id>`. Cause: <cause>`` (D3).
//!
//! Undeclared keys are dropped (Kestra drops them with a WARN).
//!
//! # Not here yet
//!
//! ARRAY, MULTISELECT, JSON, YAML and ION, `dependsOn`, SELECT `expression` and
//! `autoSelectFirst` are Task A3: until then their posted value passes through
//! unchanged (after the required / empty-string steps). FILE is checked by
//! `storage::inputs::check_file_inputs`, FORM and REUSABLE_INPUTS are not
//! supported: all three pass through unchanged.
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

use std::sync::LazyLock;

use serde_json::{Map, Value};

use crate::models::{Input, InputType};

/// The first input that failed, with Kestra's full message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputError {
    pub input_id: String,
    /// The whole text, e.g. ``Invalid value for input `n`. Cause: it must be
    /// more than `1` `` or `Missing required input:n`.
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

/// Check `posted` against `inputs` and return the coerced object, or the
/// first error in declared order (spec D1).
///
/// `posted` that is not an object is treated as `{}`.
pub fn resolve(inputs: &[Input], posted: &Value) -> Result<Map<String, Value>, InputError> {
    let posted = posted.as_object();
    let mut out = Map::new();
    for input in inputs {
        match check_input(input, posted.and_then(|m| m.get(&input.id))) {
            Ok(Some(v)) => {
                out.insert(input.id.clone(), v);
            }
            Ok(None) => {}
            Err(message) => {
                return Err(InputError {
                    input_id: input.id.clone(),
                    message,
                })
            }
        }
    }
    Ok(out)
}

/// One input's whole decision: `Ok(Some(v))` stores `v`, `Ok(None)` leaves
/// the key out, `Err` is the full message. The unit `resolve` folds over, and
/// the one a collect-every-error variant (Task B1) will fold over too.
pub(crate) fn check_input(input: &Input, posted: Option<&Value>) -> Result<Option<Value>, String> {
    if matches!(input.r#type, InputType::Form | InputType::ReusableInputs) {
        return Ok(posted.filter(|v| !v.is_null()).cloned());
    }
    let value = posted.filter(|v| match v {
        Value::Null => false,
        Value::String(s) if s.is_empty() => is_text_type(input.r#type),
        _ => true,
    });
    let Some(value) = value else {
        if let Some(default) = &input.defaults {
            let source = default.source();
            if !is_templated(source) {
                parse_and_validate(input, &Value::String(source.to_string()))
                    .map_err(|cause| invalid(input, &cause))?;
            }
            return Ok(None);
        }
        if input.required {
            return Err(format!("Missing required input:{}", input.id));
        }
        return Ok(None);
    };
    parse_and_validate(input, value)
        .map(Some)
        .map_err(|cause| invalid(input, &cause))
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

/// `parseType` then `Input.validate`, giving the typed value or the cause.
fn parse_and_validate(input: &Input, value: &Value) -> Result<Value, String> {
    match input.r#type {
        InputType::String => {
            let s = java_to_string(value);
            check_validator(input.validator.as_deref(), &s)?;
            Ok(Value::String(s))
        }
        InputType::Secret => {
            let s = java_to_string(value);
            // Thrown from inside `parseType` as a ConstraintViolationException,
            // whose message is `<path>: <msg>` (survey §1.2 (b)).
            check_validator(input.validator.as_deref(), &s)
                .map_err(|cause| format!("{}: {cause}", input.id))?;
            Ok(Value::String(s))
        }
        InputType::Email => {
            let s = java_to_string(value);
            if !EMAIL.is_match(&s) {
                return Err("The input must be a valid email".into());
            }
            Ok(Value::String(s))
        }
        InputType::Select => {
            let s = java_to_string(value);
            check_select(input, &s)?;
            Ok(Value::String(s))
        }
        InputType::Int => {
            let n = to_int(value)?;
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
            Ok(Value::from(n))
        }
        InputType::Float => {
            let x = to_float(value)?;
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
            Ok(float_value(x))
        }
        InputType::Boolean => match value {
            Value::Bool(b) => Ok(Value::Bool(*b)),
            Value::String(s) if s.eq_ignore_ascii_case("true") => Ok(Value::Bool(true)),
            Value::String(s) if s.eq_ignore_ascii_case("false") => Ok(Value::Bool(false)),
            other => Err(cannot_convert(other, "Boolean")),
        },
        InputType::Datetime => {
            let t = to_bounded(value, "Instant", parse_instant)?;
            check_after_before(
                &t,
                input.after.as_deref(),
                input.before.as_deref(),
                "Instant",
                parse_instant,
                format_instant,
            )?;
            Ok(Value::String(format_instant(&t)))
        }
        InputType::Date => {
            let d = to_bounded(value, "LocalDate", parse_date)?;
            let fmt = |d: &chrono::NaiveDate| d.format("%Y-%m-%d").to_string();
            check_after_before(
                &d,
                input.after.as_deref(),
                input.before.as_deref(),
                "LocalDate",
                parse_date,
                fmt,
            )?;
            Ok(Value::String(fmt(&d)))
        }
        InputType::Time => {
            let t = to_bounded(value, "LocalTime", parse_time)?;
            check_after_before(
                &t,
                input.after.as_deref(),
                input.before.as_deref(),
                "LocalTime",
                parse_time,
                format_time,
            )?;
            Ok(Value::String(format_time(&t)))
        }
        InputType::Duration => {
            let d = to_bounded(value, "Duration", JavaDuration::parse)?;
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
            Ok(Value::String(d.to_string()))
        }
        InputType::Uri => {
            let s = java_to_string(value);
            check_uri(&s)?;
            Ok(Value::String(s))
        }
        // Task A3 (collections, structured text) and the types checked
        // elsewhere or not supported: unchanged.
        InputType::Array
        | InputType::Multiselect
        | InputType::Json
        | InputType::Yaml
        | InputType::Ion
        | InputType::File
        | InputType::Form
        | InputType::ReusableInputs => Ok(value.clone()),
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
/// D4), unless `allowCustomValue`. With no `values` (an `expression`, Task
/// A3) nothing is checked here.
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

    /// An input from YAML, the way a flow declares it.
    fn input(yaml: &str) -> Input {
        serde_norway::from_str(yaml).unwrap_or_else(|e| panic!("{yaml}: {e}"))
    }

    /// One input checked alone: the stored value, or the message.
    fn one(yaml: &str, posted: Value) -> Result<Option<Value>, String> {
        let i = input(yaml);
        let id = i.id.clone();
        resolve(std::slice::from_ref(&i), &json!({ id.clone(): posted }))
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
        let err = resolve(&[i], &json!({})).unwrap_err();
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
        let ok = resolve(&inputs, &json!({"a": "1", "b": 7, "c": 3, "zz": 1})).unwrap();
        assert_eq!(Value::Object(ok), json!({"a": 1, "b": 7, "c": 3}));

        // b and c are both bad: b is reported, it is declared first.
        let err = resolve(&inputs, &json!({"a": 1, "b": 1})).unwrap_err();
        assert_eq!(err.input_id, "b");
        assert_eq!(
            err.message,
            "Invalid value for input `b`. Cause: it must be more than `5`"
        );
    }

    #[test]
    fn a_posted_value_that_is_not_an_object_is_empty() {
        let i = input("id: n\ntype: INT\nrequired: false\n");
        assert_eq!(resolve(&[i], &json!([1, 2])).unwrap(), Map::new());
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
                    Ok(json!("2023-06-01T10:00:00.500Z")),
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
                (json!("10:00:00"), Ok(json!("10:00"))),
                (json!("10:00:01.5"), Ok(json!("10:00:01.500"))),
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
                (json!("pt15s"), Ok(json!("PT15S"))),
                (json!("PT1S"), Err("It must be more than `PT10S`")),
                (json!("PT30S"), Err("It must be less than `PT20S`")),
            ],
        );
        table(
            "id: d\ntype: DURATION\n",
            &[
                (json!("P1D"), Ok(json!("PT24H"))),
                (json!("PT90M"), Ok(json!("PT1H30M"))),
                (json!("PT0.5S"), Ok(json!("PT0.5S"))),
                (json!("-PT0.5S"), Ok(json!("PT-0.5S"))),
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

    // --- pass-through types (A3 / elsewhere) ---------------------------------

    #[test]
    fn types_left_to_later_tasks_pass_through() {
        for (t, v) in [
            ("ARRAY\nitemType: INT", json!([1, 2])),
            ("MULTISELECT\nvalues: [A]", json!(["Z"])),
            ("JSON", json!("{\"a\":1}")),
            ("YAML", json!("a: 1")),
            ("ION", json!("{a:1}")),
            ("FILE", json!("kestra:///x/y.txt")),
        ] {
            let y = format!("id: x\ntype: {t}\n");
            assert_eq!(one(&y, v.clone()), Ok(Some(v)), "{t}");
        }
        // FILE is still required (Kestra checks `required` for every type).
        missing("id: x\ntype: FILE\n", Value::Null);
        // FORM / REUSABLE_INPUTS: never required here.
        absent("id: x\ntype: FORM\n", Value::Null);
    }
}
