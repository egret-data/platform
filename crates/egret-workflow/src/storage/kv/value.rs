//! A KV value's type and the bytes Kestra writes for it (W9c spec §4.2).
//!
//! - [`KvType`]: the seven types, stored with every revision
//!   (`workflow.kv_entries.value_type`), never re-inferred on read.
//! - [`encode`]: the object's bytes, Ion text byte-equal to what the artifact
//!   writes (w9c `types.<T>.ion_bytes`).
//! - [`decode`]: the bytes back to the JSON value a reader sees, DATETIME,
//!   DATE and DURATION as the strings Kestra's `kv()` prints
//!   (w9c `types.<T>.kv_prints`, `types._rule_read`).
//! - [`parse_ttl`]: a `ttl`, judged as the task or as the API's header does.

use serde_json::Value;

/// The type of a stored value: the column's spelling and Kestra's `KVType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KvType {
    String,
    Number,
    Boolean,
    Datetime,
    Date,
    Duration,
    Json,
}

impl KvType {
    pub const ALL: [KvType; 7] = [
        KvType::String,
        KvType::Number,
        KvType::Boolean,
        KvType::Datetime,
        KvType::Date,
        KvType::Duration,
        KvType::Json,
    ];

    /// "STRING" … "JSON": the column's spelling and Kestra's enum names.
    pub fn as_str(self) -> &'static str {
        match self {
            KvType::String => "STRING",
            KvType::Number => "NUMBER",
            KvType::Boolean => "BOOLEAN",
            KvType::Datetime => "DATETIME",
            KvType::Date => "DATE",
            KvType::Duration => "DURATION",
            KvType::Json => "JSON",
        }
    }

    /// A `kvType` property value, as Kestra's enum binding accepts it: the
    /// enum name exactly, upper case (w9c `set_coercion.type_lower`: `number`
    /// is refused; `type_bad`: `INTEGER` is refused).
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == s)
    }

    /// The type of a value after coercion (§4.2): string -> STRING, number ->
    /// NUMBER, boolean -> BOOLEAN, object or array -> JSON, `null` -> STRING
    /// (Kestra stores a rendered `null` as Ion `null`, typed STRING: w9c
    /// `typed_render.tr_null`). Never DURATION, DATETIME or DATE: those come
    /// only from `coerce` of a String with that kvType.
    pub fn of(v: &Value) -> Self {
        match v {
            Value::Null | Value::String(_) => KvType::String,
            Value::Number(_) => KvType::Number,
            Value::Bool(_) => KvType::Boolean,
            Value::Array(_) | Value::Object(_) => KvType::Json,
        }
    }
}

impl std::fmt::Display for KvType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A value as egret stores and reads it back: DATETIME, DATE and DURATION
/// are JSON strings in the form w9c `types.<T>.kv_prints` measured (§4.2, §15).
///
/// # Numbers
///
/// This crate's `serde_json` is `arbitrary_precision`, so a NUMBER keeps the
/// text it was built from:
///
/// - an integer text (`42`, `-7`, `123456789012345678901234567890`) is an Ion
///   int, exact at any size;
/// - any other text (`1.5`, `100.0`, `1e20`, `1.5e+20`) is a Java `Double`,
///   written as Ion float text (`1.5e0`, `100e0`, `1.0e20`): what Kestra's
///   Jackson makes of a JSON or rendered number;
/// - EXCEPT a text ending `e+0` (`1.50e+0`), which is an Ion DECIMAL: only the
///   API's `PUT` stores one (w9c `api.put_bodies.ion_float`, `dec_150`,
///   `nested_dec`), built with [`ion_decimal_number`], and it is written as
///   its coefficient without trailing zeros (`1.5`). `Number::from_f64`
///   never spells an exponent `e+`; `serde_json` parsing does only for JSON
///   text that writes an exponent, and the one it maps to `e+0` (`1.5e0`,
///   `1.5E+0`) is the known corner where a flow's JSON text would be stored
///   as a decimal (the same value read back, other bytes);
/// - the strings `NaN`, `Infinity` and `-Infinity` as a top-level NUMBER are
///   Ion's `nan`, `+inf` and `-inf` (w9c `api.put_bodies.ion_nan`), as
///   [`decode`] reads them back.
#[derive(Clone, Debug, PartialEq)]
pub struct KvValue {
    pub kind: KvType,
    pub json: Value,
}

/// The object's bytes, byte-equal to w9c `types.<T>.ion_bytes`: Ion text
/// through `runners::ionfmt::write_ion_json` for every type (a DURATION is an
/// Ion string, `"P1D"` with its quotes), except a DATETIME or DATE, written as
/// the Ion timestamp literal the fixture shows (UTC, millisecond precision; a
/// DATE at day precision), and an API decimal (see [`KvValue`]).
pub fn encode(v: &KvValue) -> Vec<u8> {
    let mut out = String::new();
    match (v.kind, &v.json) {
        (KvType::Datetime, Value::String(s)) if write_datetime(&mut out, s) => {}
        (KvType::Date, Value::String(s)) if write_date(&mut out, s) => {}
        (KvType::Number, Value::String(s)) if write_special_float(&mut out, s) => {}
        _ => write_json(&mut out, &v.json),
    }
    out.into_bytes()
}

/// `write_ion_json`, with an API decimal (a number text ending `e+0`, see
/// [`KvValue`]) at any depth written as an Ion decimal.
fn write_json(out: &mut String, v: &Value) {
    if !holds_decimal(v) {
        crate::runners::write_ion_json(out, v);
        return;
    }
    match v {
        Value::Number(n) => match ion_decimal(&n.to_string()) {
            Some(text) => out.push_str(&text),
            None => crate::runners::write_ion_json(out, v),
        },
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_json(out, item);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (i, (name, item)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                crate::runners::write_ion_symbol(out, name);
                out.push(':');
                write_json(out, item);
            }
            out.push('}');
        }
        other => crate::runners::write_ion_json(out, other),
    }
}

fn holds_decimal(v: &Value) -> bool {
    match v {
        Value::Number(n) => ion_decimal(&n.to_string()).is_some(),
        Value::Array(items) => items.iter().any(holds_decimal),
        Value::Object(map) => map.values().any(holds_decimal),
        _ => false,
    }
}

/// An Ion decimal as a NUMBER [`encode`] writes as one: `plain` is
/// `-?[0-9]+(\.[0-9]+)?` (`1.50`, `0.1`, `-2`), the API body's decimal in
/// plain notation (Task 6 normalises `1.5d0` and `15d-1` to it). `None` for
/// any other text.
pub fn ion_decimal_number(plain: &str) -> Option<serde_json::Number> {
    let digits = plain.strip_prefix('-').unwrap_or(plain);
    let (int, frac) = digits.split_once('.').unwrap_or((digits, "0"));
    let ok = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !ok(int) || !ok(frac) {
        return None;
    }
    serde_json::from_str(&format!("{plain}e0")).ok()
}

/// `1.50e+0` -> `1.5`, `2e+0` -> `2.`, `-0.10e+0` -> `-0.1`: the coefficient
/// without trailing zeros after the point, as the artifact stores an API
/// decimal (w9c `api.put_bodies.dec_150`: `1.50` -> `1.5`). `None` for a text
/// that is not [`ion_decimal_number`]'s.
fn ion_decimal(text: &str) -> Option<String> {
    let mantissa = text.strip_suffix("e+0")?;
    let (sign, unsigned) = match mantissa.strip_prefix('-') {
        Some(m) => ("-", m),
        None => ("", mantissa),
    };
    let (int, frac) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    if int.is_empty() || !int.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(format!("{sign}{int}.{}", frac.trim_end_matches('0')))
}

/// A top-level NUMBER that is not finite: Ion's spelling.
fn write_special_float(out: &mut String, s: &str) -> bool {
    let text = match s {
        "NaN" => "nan",
        "Infinity" => "+inf",
        "-Infinity" => "-inf",
        _ => return false,
    };
    out.push_str(text);
    true
}

/// An RFC 3339 instant as an Ion timestamp in UTC at millisecond precision
/// (w9c `types.dt*`: `2025-01-02T03:04:05.000Z`; nanoseconds truncated).
fn write_datetime(out: &mut String, s: &str) -> bool {
    let Ok(t) = chrono::DateTime::parse_from_rfc3339(s) else {
        return false;
    };
    let t = t.with_timezone(&chrono::Utc);
    out.push_str(&t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string());
    true
}

/// An ISO date as a day-precision Ion timestamp (w9c `types.date`:
/// `2025-01-02`).
fn write_date(out: &mut String, s: &str) -> bool {
    let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") else {
        return false;
    };
    out.push_str(&d.format("%Y-%m-%d").to_string());
    true
}

/// The bytes back to the JSON value of `kind`: exactly one Ion value through
/// `storage::ion::read_values_capped` (bounded by `cap`), then normalised for
/// DATETIME/DATE as [`encode`] wrote them and for DURATION as w9c
/// `types._rule_read` prints it. A null inside a map is dropped, as every
/// Kestra read drops it (w9c `types.json_nested`). Err names what is wrong,
/// never a storage key.
pub fn decode(kind: KvType, bytes: &[u8], cap: u64) -> Result<Value, String> {
    use crate::storage::ion::{read_values_capped, Stop};
    let (mut values, stop) = read_values_capped(bytes, 1, cap);
    match stop {
        Stop::End => {}
        Stop::Full => {
            return Err(format!(
                "the stored {kind} value holds more than one Ion value"
            ))
        }
        Stop::Failed(e) | Stop::TooDeep(e) | Stop::TooLarge(e) | Stop::Crashed(e) => {
            return Err(format!("the stored {kind} value cannot be read: {e}"))
        }
    }
    let Some(v) = values.pop() else {
        return Err(format!("the stored {kind} value holds no Ion value"));
    };
    let mismatch = |v: &Value| {
        let what = match v {
            Value::Null => "null",
            Value::Bool(_) => "a boolean",
            Value::Number(_) => "a number",
            Value::String(_) => "a string",
            Value::Array(_) => "a list",
            Value::Object(_) => "a struct",
        };
        format!("the stored {kind} value is {what}, not a {kind}")
    };
    match (kind, v) {
        // A rendered `null` is stored as Ion `null`, typed STRING (`KvType::of`).
        (KvType::String, v @ (Value::String(_) | Value::Null)) => Ok(v),
        (KvType::Number, v @ Value::Number(_)) => Ok(v),
        // `storage::ion`'s spelling of a float that is not finite.
        (KvType::Number, Value::String(s))
            if matches!(s.as_str(), "NaN" | "Infinity" | "-Infinity") =>
        {
            Ok(Value::String(s))
        }
        (KvType::Boolean, v @ Value::Bool(_)) => Ok(v),
        (KvType::Json, v @ (Value::Object(_) | Value::Array(_))) => Ok(v),
        (KvType::Datetime, Value::String(s)) => chrono::DateTime::parse_from_rfc3339(&s)
            .map(|t| Value::String(java_instant(t.with_timezone(&chrono::Utc))))
            .map_err(|_| format!("the stored DATETIME value '{s}' is not an instant")),
        (KvType::Date, Value::String(s)) => chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d")
            .map(|d| Value::String(d.format("%Y-%m-%d").to_string()))
            .map_err(|_| format!("the stored DATE value '{s}' is not a date")),
        (KvType::Duration, Value::String(s)) => Ok(Value::String(kestra_duration(&s).unwrap_or(s))),
        (_, v) => Err(mismatch(&v)),
    }
}

/// `java.time.Instant.toString`: seconds always, the fraction in groups of
/// three digits and only when non-zero (w9c `types.dt`: `…:05Z`,
/// `types.dt_millis`: `…:05.123Z`).
fn java_instant(t: chrono::DateTime<chrono::Utc>) -> String {
    use chrono::Timelike as _;
    let nanos = t.nanosecond() % 1_000_000_000;
    let base = t.format("%Y-%m-%dT%H:%M:%S").to_string();
    let frac = if nanos == 0 {
        String::new()
    } else if nanos.is_multiple_of(1_000_000) {
        format!(".{:03}", nanos / 1_000_000)
    } else if nanos.is_multiple_of(1_000) {
        format!(".{:06}", nanos / 1_000)
    } else {
        format!(".{nanos:09}")
    };
    format!("{base}{frac}Z")
}

/// w9c `types._rule_read`: a text matching Kestra's DURATION_PATTERN
/// `^P(?=[^T]|T.)(?:\d*D)?(?:T(?=.)(?:\d*H)?(?:\d*M)?(?:\d*S)?)?$` prints as
/// `java.time.Duration.parse(text).toString()` (`P1D` -> `PT24H`); `None` for
/// any other text (`PT1.5S`, `p1d`, `-PT1S`), which prints as written, and
/// for one `Duration.parse` would refuse (`PD`: a unit with no digits; a
/// total past `Long` seconds).
fn kestra_duration(text: &str) -> Option<String> {
    let rest = text.strip_prefix('P')?;
    // (?=[^T]|T.)
    if rest.is_empty() || rest == "T" {
        return None;
    }
    let (date, time) = match rest.split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None => (rest, None),
    };
    /// `\d*{unit}` at the start of `s`: Some((digits, rest)) when present.
    fn unit(s: &str, unit: char) -> (Option<&str>, &str) {
        let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
        if s[end..].starts_with(unit) {
            (Some(&s[..end]), &s[end + 1..])
        } else {
            (None, s)
        }
    }
    let (days, left) = unit(date, 'D');
    if !left.is_empty() {
        return None;
    }
    let (mut hours, mut minutes, mut seconds) = (None, None, None);
    if let Some(t) = time {
        // T(?=.)
        if t.is_empty() {
            return None;
        }
        let (h, t) = unit(t, 'H');
        let (m, t) = unit(t, 'M');
        let (s, t) = unit(t, 'S');
        if !t.is_empty() {
            return None;
        }
        (hours, minutes, seconds) = (h, m, s);
    }
    let mut total: i64 = 0;
    for (digits, per) in [
        (days, 86_400i64),
        (hours, 3_600),
        (minutes, 60),
        (seconds, 1),
    ] {
        let Some(digits) = digits else { continue };
        // `Duration.parse` refuses a unit with no digits, and a total past a
        // long: such a text prints as written.
        let n: i64 = digits.parse().ok()?;
        total = total.checked_add(n.checked_mul(per)?)?;
    }
    Some(java_duration(total))
}

/// `java.time.Duration.toString` of a non-negative whole number of seconds.
fn java_duration(seconds: i64) -> String {
    if seconds == 0 {
        return "PT0S".into();
    }
    let (h, m, s) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    let mut out = String::from("PT");
    if h != 0 {
        out.push_str(&format!("{h}H"));
    }
    if m != 0 {
        out.push_str(&format!("{m}M"));
    }
    if s != 0 {
        out.push_str(&format!("{s}S"));
    }
    out
}

/// Where a ttl comes from: the two measured behaviours differ (Task 1 review I3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ttl {
    /// The tasks' `ttl` property (Set): zero and negative are both refused
    /// with `ttl: must be a positive duration` (w9c `ttl.PT0S`, `ttl.-PT1S`);
    /// a malformed one names the value and `ttl` (w9c `ttl.xyz`).
    Task,
    /// The API's `ttl` header: zero is ACCEPTED (the value is expired at once,
    /// w9c `api.headers.h_ttl_zero`); a negative one is `ttl cannot be
    /// negative` (`h_ttl_neg`, exact); a malformed one `Cannot convert value
    /// '<v>' to Duration` (`h_ttl_bad`, exact).
    Header,
}

/// An ISO-8601 duration as `java.time.Duration.parse` reads it
/// (`executor::parse_duration`, retry.rs), then zero and the sign judged by
/// `mode`. Err is the message for that mode.
pub fn parse_ttl(raw: &str, mode: Ttl) -> Result<chrono::TimeDelta, String> {
    // `parse_duration` refuses a sign; `-` is read here so that a negative
    // ttl gets its own message rather than the malformed one.
    let (negative, magnitude) = match raw.strip_prefix('-') {
        Some(rest) if !rest.starts_with(['-', '+']) => (true, rest),
        _ => (false, raw),
    };
    let parsed = crate::executor::parse_duration(magnitude)
        .and_then(|d| chrono::TimeDelta::from_std(d).ok());
    let Some(d) = parsed else {
        return Err(match mode {
            Ttl::Task => {
                format!("ttl: '{raw}' is not an ISO-8601 duration (for example PT1H or P1D)")
            }
            Ttl::Header => format!("Cannot convert value '{raw}' to Duration"),
        });
    };
    let d = if negative { -d } else { d };
    match mode {
        Ttl::Task if d <= chrono::TimeDelta::zero() => {
            Err("ttl: must be a positive duration".to_string())
        }
        Ttl::Header if d < chrono::TimeDelta::zero() => Err("ttl cannot be negative".to_string()),
        _ => Ok(d),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Value {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/oracle-exec/w9c-kv.json"
        )))
        .expect("the fixture parses")
    }

    /// Every `types` row: (name, the KvValue a coercion makes of it, the
    /// expected bytes, the expected read). The KvValue is built from the
    /// cell's egret answer (`api.type`, `get_value`), which is what Task 5's
    /// coercion must produce; the rows whose input coerces differently from
    /// what egret reads back (a Double) are given as Doubles.
    fn types_rows() -> Vec<(String, KvValue, String, Value)> {
        let f = fixture();
        let types = f["egret_expected"]["types"].as_object().unwrap().clone();
        let mut rows = vec![];
        for (name, cell) in types {
            if name.starts_with('_') {
                continue;
            }
            let kind = KvType::parse(cell["api"]["type"].as_str().unwrap()).unwrap();
            let get = cell["get_value"].clone();
            let json = match kind {
                // A Double (Jackson's read of a non-integer number): the
                // number of the read, which is f64-backed.
                KvType::Number
                    if !cell["ion_bytes"]
                        .as_str()
                        .unwrap()
                        .bytes()
                        .all(|b| b.is_ascii_digit() || b == b'-') =>
                {
                    let f = get.as_f64().unwrap();
                    Value::Number(serde_json::Number::from_f64(f).unwrap())
                }
                // A JSON value: the set input as Jackson parses it (keeps
                // the nulls inside maps the read drops).
                KvType::Json => {
                    let input = cell["set"]["value"].as_str().unwrap();
                    serde_json::from_str(input).unwrap()
                }
                // A DURATION is stored as written.
                KvType::Duration => cell["set"]["value"].clone(),
                _ => get.clone(),
            };
            rows.push((
                name,
                KvValue { kind, json },
                cell["ion_bytes"].as_str().unwrap().to_string(),
                get,
            ));
        }
        rows
    }

    /// JSON equality with numbers compared by value (`100.0` == `1e2`), as
    /// the cells' `compare` says ("JSON-equal").
    fn json_eq(a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Number(x), Value::Number(y)) => {
                x.to_string() == y.to_string() || x.as_f64() == y.as_f64()
            }
            (Value::Array(x), Value::Array(y)) => {
                x.len() == y.len() && x.iter().zip(y).all(|(a, b)| json_eq(a, b))
            }
            (Value::Object(x), Value::Object(y)) => {
                x.len() == y.len()
                    && x.iter()
                        .all(|(k, v)| y.get(k).is_some_and(|w| json_eq(v, w)))
            }
            _ => a == b,
        }
    }

    #[test]
    fn encode_is_byte_equal_to_the_artifact_for_every_type() {
        let rows = types_rows();
        assert_eq!(rows.len(), 39, "every types row is examined");
        let mut kinds = std::collections::BTreeSet::new();
        for (name, v, bytes, _) in &rows {
            assert_eq!(String::from_utf8(encode(v)).unwrap(), *bytes, "{name}");
            kinds.insert(v.kind);
        }
        assert_eq!(kinds.len(), 7, "every type is covered: {kinds:?}");
    }

    #[test]
    fn decode_reads_back_what_encode_wrote_for_every_type() {
        for (name, v, _, get) in types_rows() {
            let back = decode(v.kind, &encode(&v), 1 << 20).unwrap();
            assert!(json_eq(&back, &get), "{name}: {back} vs {get}");
        }
    }

    #[test]
    fn decode_of_the_artifacts_bytes_answers_what_kestras_kv_printed() {
        let f = fixture();
        let types = f["egret_expected"]["types"].as_object().unwrap();
        let mut examined = 0;
        for (name, cell) in types {
            if name.starts_with('_') {
                continue;
            }
            let kind = KvType::parse(cell["api"]["type"].as_str().unwrap()).unwrap();
            let bytes = cell["ion_bytes"].as_str().unwrap();
            let got = decode(kind, bytes.as_bytes(), 1 << 20).unwrap();
            // The egret answer: get_value JSON-equal, and the API's value.
            assert!(json_eq(&got, &cell["get_value"]), "{name}: {got}");
            assert!(json_eq(&got, &cell["api"]["value"]), "{name}: {got}");
            // A DATETIME, DATE or DURATION reads as the string kv() prints.
            if matches!(kind, KvType::Datetime | KvType::Date | KvType::Duration) {
                assert_eq!(
                    got.as_str().unwrap(),
                    cell["kv_prints"].as_str().unwrap(),
                    "{name}"
                );
            }
            examined += 1;
        }
        assert_eq!(examined, 39);
    }

    #[test]
    fn a_duration_is_stored_as_an_ion_string() {
        let v = KvValue {
            kind: KvType::Duration,
            json: Value::String("P1D".into()),
        };
        assert_eq!(encode(&v), b"\"P1D\"");
        assert_eq!(decode(KvType::Duration, b"\"P1D\"", 4096).unwrap(), "PT24H");
        // Not Kestra's pattern: as written (w9c set_coercion.dur_lower).
        for raw in ["p1d", "-PT1S", "PT1.5S", "PD", "PT", "P", "P1DT", "P1H"] {
            let bytes = encode(&KvValue {
                kind: KvType::Duration,
                json: Value::String(raw.into()),
            });
            assert_eq!(
                decode(KvType::Duration, &bytes, 4096).unwrap(),
                raw,
                "{raw}"
            );
        }
        for (raw, printed) in [
            ("PT36H", "PT36H"),
            ("P1DT2H", "PT26H"),
            ("PT90M", "PT1H30M"),
            ("PT0S", "PT0S"),
            ("PT61S", "PT1M1S"),
            ("P0D", "PT0S"),
        ] {
            assert_eq!(kestra_duration(raw).as_deref(), Some(printed), "{raw}");
        }
    }

    #[test]
    fn decode_refuses_two_values_and_none() {
        for kind in KvType::ALL {
            let err = decode(kind, b"1 2", 4096).unwrap_err();
            assert!(err.contains("more than one"), "{err}");
            let err = decode(kind, b"", 4096).unwrap_err();
            assert!(err.contains("no Ion value"), "{err}");
            let err = decode(kind, b"  ", 4096).unwrap_err();
            assert!(err.contains("no Ion value"), "{err}");
            let err = decode(kind, b"{a:", 4096).unwrap_err();
            assert!(err.contains("cannot be read"), "{err}");
        }
        // The type is the stored one: a mismatch is named, not re-inferred.
        let err = decode(KvType::Number, b"\"x\"", 4096).unwrap_err();
        assert_eq!(err, "the stored NUMBER value is a string, not a NUMBER");
        assert!(decode(KvType::Boolean, b"1", 4096).is_err());
        assert!(decode(KvType::Json, b"1", 4096).is_err());
        assert!(decode(KvType::Datetime, b"\"x\"", 4096).is_err());
        // Bounded by the cap.
        let big = format!("\"{}\"", "x".repeat(20_000));
        assert!(decode(KvType::String, big.as_bytes(), 4096).is_err());
        assert!(decode(KvType::String, big.as_bytes(), 40_000).is_ok());
    }

    #[test]
    fn the_api_decimals_and_non_finite_numbers_are_byte_equal_to_the_artifact() {
        // w9c api.put_bodies: ion_float `1.5`, dec_150 `1.50` -> `1.5`,
        // dec_0_1 `0.1`, ion_decimal_d `1.5d0` -> `1.5`, nested_dec, ion_nan.
        let num = |t: &str| Value::Number(serde_json::from_str(t).unwrap());
        let dec = |t: &str| Value::Number(ion_decimal_number(t).unwrap());
        for (json, bytes) in [
            (dec("1.5"), "1.5"),
            (dec("1.50"), "1.5"),
            (dec("0.1"), "0.1"),
            (dec("-0.025"), "-0.025"),
            (dec("2"), "2."),
            (dec("200.00"), "200."),
            (
                dec("123456789012345678901234567890.5"),
                "123456789012345678901234567890.5",
            ),
        ] {
            let v = KvValue {
                kind: KvType::Number,
                json,
            };
            assert_eq!(String::from_utf8(encode(&v)).unwrap(), bytes);
        }
        for bad in ["1e5", "", "-", ".5", "5.", "1.5d0", "a"] {
            assert!(ion_decimal_number(bad).is_none(), "{bad}");
        }
        // The controls: Doubles of the same values stay Ion floats.
        for (json, bytes) in [
            (num("1.5"), "1.5e0"),
            (num("1.5e20"), "1.5e20"),
            (num("1e-7"), "1.0e-7"),
        ] {
            let v = KvValue {
                kind: KvType::Number,
                json,
            };
            assert_eq!(String::from_utf8(encode(&v)).unwrap(), bytes);
        }
        let nested = KvValue {
            kind: KvType::Json,
            json: serde_json::json!({"a": {"b": dec("1.5")}, "c d": [num("1.5"), 1]}),
        };
        assert_eq!(
            String::from_utf8(encode(&nested)).unwrap(),
            "{a:{b:1.5},'c d':[1.5e0,1]}"
        );
        let nan = KvValue {
            kind: KvType::Number,
            json: Value::String("NaN".into()),
        };
        assert_eq!(encode(&nan), b"nan");
        assert_eq!(decode(KvType::Number, b"nan", 4096).unwrap(), "NaN");
        assert_eq!(
            decode(KvType::Number, b"1.5", 4096).unwrap(),
            serde_json::json!(1.5)
        );
    }

    #[test]
    fn the_type_of_a_value_and_the_kvtype_spelling() {
        use serde_json::json;
        for (v, t) in [
            (json!("s"), KvType::String),
            (json!(null), KvType::String),
            (json!(1), KvType::Number),
            (json!(1.5), KvType::Number),
            (json!(true), KvType::Boolean),
            (json!({}), KvType::Json),
            (json!([]), KvType::Json),
        ] {
            assert_eq!(KvType::of(&v), t, "{v}");
        }
        for t in KvType::ALL {
            assert_eq!(KvType::parse(t.as_str()), Some(t));
        }
        for bad in ["number", "Number", "INTEGER", "", " STRING"] {
            assert_eq!(KvType::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn parse_ttl_task_refuses_zero_and_negative_as_not_positive() {
        let f = fixture();
        let ttl = f["egret_expected"]["ttl"].as_object().unwrap();
        let mut examined = 0;
        for (name, cell) in ttl {
            let Some(raw) = cell.get("ttl").and_then(Value::as_str) else {
                continue;
            };
            if raw.contains("{{") {
                continue; // rendered first (Task 5): `expr`
            }
            examined += 1;
            let got = parse_ttl(raw, Ttl::Task);
            match cell["outcome"].as_str().unwrap() {
                "ok" => assert_eq!(
                    got.unwrap().num_seconds(),
                    cell["expires_after_write_s"].as_i64().unwrap(),
                    "{name}"
                ),
                _ => {
                    let err = got.unwrap_err();
                    if let Some(m) = cell["message"].as_str() {
                        assert_eq!(err, m, "{name}");
                    }
                    for frag in cell["message_contains"].as_array().into_iter().flatten() {
                        assert!(err.contains(frag.as_str().unwrap()), "{name}: {err}");
                    }
                }
            }
        }
        assert_eq!(examined, 6, "PT0S, -PT1S, PT2S, P1D, xyz, P7D");
        assert_eq!(
            parse_ttl("PT0S", Ttl::Task).unwrap_err(),
            "ttl: must be a positive duration"
        );
        assert_eq!(
            parse_ttl("-PT1S", Ttl::Task).unwrap_err(),
            "ttl: must be a positive duration"
        );
        assert_eq!(parse_ttl("PT1H", Ttl::Task).unwrap().num_seconds(), 3600);
    }

    #[test]
    fn parse_ttl_header_accepts_zero_and_refuses_negative_with_its_own_message() {
        let f = fixture();
        let headers = &f["egret_expected"]["api"]["headers"];
        for name in ["h_ttl_zero", "h_ttl_neg", "h_ttl_bad", "h_ttl", "h_both"] {
            let cell = &headers[name];
            let raw = cell["ttl"].as_str().unwrap();
            let got = parse_ttl(raw, Ttl::Header);
            if cell["status"] == 200 {
                assert!(got.is_ok(), "{name}: {got:?}");
            } else {
                assert_eq!(
                    got.unwrap_err(),
                    cell["message"].as_str().unwrap(),
                    "{name}"
                );
            }
        }
        assert_eq!(
            parse_ttl("PT0S", Ttl::Header).unwrap(),
            chrono::TimeDelta::zero()
        );
        assert_eq!(
            parse_ttl("-PT1S", Ttl::Header).unwrap_err(),
            "ttl cannot be negative"
        );
        assert_eq!(
            parse_ttl("xyz", Ttl::Header).unwrap_err(),
            "Cannot convert value 'xyz' to Duration"
        );
        assert_eq!(
            parse_ttl("--PT1S", Ttl::Header).unwrap_err(),
            "Cannot convert value '--PT1S' to Duration"
        );
    }
}
