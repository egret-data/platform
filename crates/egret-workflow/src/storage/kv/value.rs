//! A KV value's type and the bytes Kestra writes for it (W9c spec §4.2).
//!
//! - [`KvType`]: the seven types, stored with every revision
//!   (`workflow.kv_entries.value_type`), never re-inferred on read.
//! - [`KvData`]: a value as the KV store holds it. Every number keeps its Ion
//!   kind and its exact digits ([`KvNumber`]: the user's ruling, 2026-10-07,
//!   "decimals are preserved exactly").
//! - [`encode`]: the object's bytes, Ion text byte-equal to what the artifact
//!   writes (w9c `types.<T>.ion_bytes`).
//! - [`decode`]: the bytes back to a [`KvData`], DATETIME, DATE and DURATION
//!   as the strings Kestra's `kv()` prints (w9c `types.<T>.kv_prints`,
//!   `types._rule_read`). [`check_readable`] is the write rule built on it.
//! - [`unmodelled`]: what the KV store does not model (a blob, a clob, an
//!   s-expression, an annotation), found at any depth.
//! - [`parse_ttl`]: a `ttl`, judged as the task or as the API's header does.

use std::ops::ControlFlow;

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

    /// [`KvType::of`] for a [`KvData`]: the type of `Put`'s merged result
    /// (Task 3), named by the same rule (a `null` is STRING).
    pub fn of_data(d: &KvData) -> Self {
        match d {
            KvData::Null | KvData::String(_) => KvType::String,
            KvData::Number(_) => KvType::Number,
            KvData::Bool(_) => KvType::Boolean,
            KvData::List(_) | KvData::Struct(_) => KvType::Json,
        }
    }
}

impl std::fmt::Display for KvType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One number of a stored value, with its Ion kind and its exact digits (the
/// user's ruling, 2026-10-07: "decimals are preserved exactly").
///
/// - A number that arrives as an Ion **decimal** (only the API's `PUT` makes
///   one) stays a decimal through every read, merge and write, no digit lost.
/// - A **float** stays a float: a Java `Double`, as Kestra's Jackson makes of
///   any JSON or rendered number with a fraction or an exponent.
/// - An **int** stays an int, exact at any size.
#[derive(Clone, Debug, PartialEq)]
pub enum KvNumber {
    /// `-?[0-9]+`, exact at any size: an Ion int.
    Int(String),
    /// An Ion decimal as `java.math.BigDecimal.toPlainString` writes it:
    /// `-?[0-9]+(\.[0-9]+)?`, its scale kept, never an exponent, no negative
    /// zero (a `BigDecimal` has none). This is also the text a reader sees
    /// ([`KvData::to_json`]): no fixture cell measures how Kestra prints a
    /// decimal it read, so the plain form is egret's choice (spec §4.2).
    Decimal(String),
    /// An Ion float (a Java `Double`), NaN and the infinities included.
    Float(f64),
}

impl KvNumber {
    /// An Ion int's text: `Some` only for `-?[0-9]+` with no leading zero
    /// (`0`, `-7`; never `007`, which Ion and JSON both refuse).
    pub fn int(text: &str) -> Option<Self> {
        is_whole(text.strip_prefix('-').unwrap_or(text)).then(|| KvNumber::Int(text.into()))
    }

    /// A decimal in plain notation (`1.50`, `-0.025`, `2`): `Some` only for
    /// `-?[0-9]+(\.[0-9]+)?` whose integer part has no leading zero (`0.5`,
    /// never `00.5` or `007`). A negative zero is zero.
    pub fn decimal(plain: &str) -> Option<Self> {
        let unsigned = plain.strip_prefix('-').unwrap_or(plain);
        let (int, frac) = match unsigned.split_once('.') {
            Some((int, frac)) => (int, Some(frac)),
            None => (unsigned, None),
        };
        if !is_whole(int) || !frac.is_none_or(is_digits) {
            return None;
        }
        let zero = unsigned.bytes().all(|b| b == b'0' || b == b'.');
        Some(KvNumber::Decimal(
            if zero { unsigned } else { plain }.into(),
        ))
    }

    /// The Ion decimal `d` in plain notation ([`KvNumber::Decimal`]), its
    /// length charged to `budget` BEFORE the text is built (`1d99999999` is
    /// eleven bytes of Ion and a hundred million digits of text; forty
    /// `1d9000000` in one list would be 360 MB: review NI1).
    fn of_ion_decimal(d: &ion_rs::Decimal, budget: &mut Budget) -> Result<Self, String> {
        let coefficient = d.coefficient();
        let digits = coefficient.magnitude().to_string();
        let sign = if coefficient.is_negative() && !d.is_zero() {
            "-"
        } else {
            ""
        };
        let exponent = d.exponent();
        let len = digits.len() as u64;
        let shift = exponent.unsigned_abs();
        let plain_len = if d.is_zero() && exponent >= 0 {
            1
        } else if exponent >= 0 {
            len.saturating_add(shift)
        } else {
            len.max(shift).saturating_add(2)
        };
        budget.charge(plain_len)?;
        let text = if exponent >= 0 {
            if d.is_zero() {
                "0".to_string()
            } else {
                format!("{sign}{digits}{}", "0".repeat(shift as usize))
            }
        } else {
            let scale = shift as usize;
            if digits.len() > scale {
                let (int, frac) = digits.split_at(digits.len() - scale);
                format!("{sign}{int}.{frac}")
            } else {
                format!("{sign}0.{}{digits}", "0".repeat(scale - digits.len()))
            }
        };
        Ok(KvNumber::Decimal(text))
    }

    /// This number as a JSON number: an int or a decimal by its exact text
    /// (this crate's `serde_json` is `arbitrary_precision`), a float as
    /// `storage::ion` shows one (NaN and the infinities as the strings `NaN`,
    /// `Infinity`, `-Infinity`).
    fn to_json(&self) -> Value {
        match self {
            KvNumber::Int(text) | KvNumber::Decimal(text) => {
                serde_json::from_str::<serde_json::Number>(text)
                    .map(Value::Number)
                    .unwrap_or_else(|_| Value::String(text.clone()))
            }
            KvNumber::Float(f) if f.is_nan() => Value::String("NaN".into()),
            KvNumber::Float(f) if f.is_infinite() => {
                Value::String(if *f > 0.0 { "Infinity" } else { "-Infinity" }.into())
            }
            KvNumber::Float(f) => serde_json::Number::from_f64(*f)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        }
    }

    /// Ion text: an int as its digits; a decimal as its digits with the
    /// trailing zeros after the point dropped, the point always written
    /// (w9c `api.put_bodies.dec_150`: `1.50` is stored as `1.5`; `2` as
    /// `2.`, which Ion reads as a decimal); a float as Kestra writes a
    /// `Double` (`1.5e0`, `nan`).
    fn write(&self, out: &mut String) {
        match self {
            KvNumber::Int(text) => out.push_str(text),
            KvNumber::Decimal(text) => {
                let (int, frac) = text.split_once('.').unwrap_or((text, ""));
                out.push_str(int);
                out.push('.');
                out.push_str(frac.trim_end_matches('0'));
            }
            KvNumber::Float(f) => crate::runners::write_ion_float(out, *f),
        }
    }
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Digits with no leading zero: `0`, `10`; not `007`.
fn is_whole(s: &str) -> bool {
    is_digits(s) && (s.len() == 1 || !s.starts_with('0'))
}

/// What one Ion value may grow to as [`KvData`] (review NI1): the text of
/// every decimal's plain digits, int, string, symbol and field name it
/// copies, summed over the WHOLE value and charged before a decimal's text
/// is built. It starts at the cap (`workflow.storage.max_read_bytes`), so
/// the decoded text of a value is never more than the cap; what egret
/// writes never expands (its text is at most its encoded bytes), so every
/// value [`check_readable`] accepts stays within it.
struct Budget {
    cap: u64,
    left: u64,
}

impl Budget {
    fn new(cap: u64) -> Self {
        Budget { cap, left: cap }
    }

    fn charge(&mut self, bytes: u64) -> Result<(), String> {
        match self.left.checked_sub(bytes) {
            Some(left) => {
                self.left = left;
                Ok(())
            }
            None => Err(format!(
                "decodes to more than workflow.storage.max_read_bytes ({} bytes) of digits and \
                 text",
                self.cap
            )),
        }
    }
}

/// A value as the KV store holds it: JSON's shapes, with every number a
/// [`KvNumber`]. This is the merge-ready value: [`decode`] makes it, `Put`'s
/// merge (Task 3) works on it, [`encode`] writes it, and
/// [`KvData::to_json`] is what a reader (`kv()`, `Get`, the API) sees.
#[derive(Clone, Debug, PartialEq)]
pub enum KvData {
    Null,
    Bool(bool),
    Number(KvNumber),
    String(String),
    List(Vec<KvData>),
    /// The fields in order, each name once.
    Struct(Vec<(String, KvData)>),
}

impl KvData {
    /// A flow's value, with `serde_json`'s semantics (the user's ruling,
    /// 2026-10-07): a number written with only digits is an int, any other
    /// (a fraction, an exponent) a float, as Kestra's Jackson reads a JSON
    /// or rendered number. A flow never makes a decimal. A null inside an
    /// object is kept (Kestra writes it; every read drops it).
    pub fn from_json(v: &Value) -> Self {
        match v {
            Value::Null => KvData::Null,
            Value::Bool(b) => KvData::Bool(*b),
            Value::Number(n) => {
                let text = n.to_string();
                KvData::Number(KvNumber::int(&text).unwrap_or_else(|| {
                    // A text `serde_json` accepted but `f64` does not parse
                    // cannot occur; out of range is an infinity, as in Java.
                    KvNumber::Float(text.parse().unwrap_or(f64::NAN))
                }))
            }
            Value::String(s) => KvData::String(s.clone()),
            Value::Array(items) => KvData::List(items.iter().map(Self::from_json).collect()),
            Value::Object(map) => KvData::Struct(
                map.iter()
                    .map(|(name, item)| (name.clone(), Self::from_json(item)))
                    .collect(),
            ),
        }
    }

    /// What a reader sees: JSON, an int or a decimal by its exact text, a
    /// float as `storage::ion` shows one (see [`KvNumber::to_json`]).
    pub fn to_json(&self) -> Value {
        match self {
            KvData::Null => Value::Null,
            KvData::Bool(b) => Value::Bool(*b),
            KvData::Number(n) => n.to_json(),
            KvData::String(s) => Value::String(s.clone()),
            KvData::List(items) => Value::Array(items.iter().map(Self::to_json).collect()),
            KvData::Struct(fields) => Value::Object(
                fields
                    .iter()
                    .map(|(name, item)| (name.clone(), item.to_json()))
                    .collect(),
            ),
        }
    }

    /// One Ion value read from the store, or why the KV store cannot hold
    /// it: anything [`unmodelled`] finds, or a value whose digits and text
    /// (every decimal's plain digits, every int, string, symbol and field
    /// name) add up to more than `cap` (review NI1). A field whose value is a null is dropped, as every
    /// Kestra read drops it (w9c `types.json_nested`); a field named twice
    /// keeps its first place and its last value. A timestamp inside a value
    /// is its text, as the preview shows it (`storage::ion`).
    pub fn from_element(element: &ion_rs::Element, cap: u64) -> Result<Self, String> {
        if let Some(found) = unmodelled(element) {
            return Err(format!(
                "holds {}, which the KV store does not model",
                found.with_article()
            ));
        }
        Self::convert(element, &mut Budget::new(cap))
    }

    /// `element` as KvData, every copied text charged to `budget` (shared by
    /// the whole value, never per leaf).
    fn convert(element: &ion_rs::Element, budget: &mut Budget) -> Result<Self, String> {
        use ion_rs::Value as V;
        let text = |t: &str, budget: &mut Budget| -> Result<String, String> {
            budget.charge(t.len() as u64)?;
            Ok(t.to_string())
        };
        Ok(match element.value() {
            V::Null(_) => KvData::Null,
            V::Bool(b) => KvData::Bool(*b),
            // An int's digits are at most a few times the bytes that hold it
            // in the element already; charged once built.
            V::Int(i) => {
                let digits = i.to_string();
                budget.charge(digits.len() as u64)?;
                KvData::Number(KvNumber::Int(digits))
            }
            V::Float(f) => KvData::Number(KvNumber::Float(*f)),
            V::Decimal(d) => KvData::Number(KvNumber::of_ion_decimal(d, budget)?),
            V::Timestamp(t) => {
                let shown = crate::storage::ion::timestamp_text(t);
                budget.charge(shown.len() as u64)?;
                KvData::String(shown)
            }
            V::Symbol(s) => match s.text() {
                Some(t) => KvData::String(text(t, budget)?),
                None => KvData::Null,
            },
            V::String(s) => KvData::String(text(s.text(), budget)?),
            V::List(seq) => KvData::List(
                seq.elements()
                    .map(|item| Self::convert(item, budget))
                    .collect::<Result<_, _>>()?,
            ),
            V::Struct(st) => {
                let mut fields: Vec<(String, KvData)> = Vec::new();
                let mut at = std::collections::HashMap::new();
                for (name, item) in st.fields() {
                    if item.is_null() {
                        continue;
                    }
                    let name = text(name.text().unwrap_or(""), budget)?;
                    let item = Self::convert(item, budget)?;
                    match at.get(&name) {
                        Some(&i) => fields[i] = (name, item),
                        None => {
                            at.insert(name.clone(), fields.len());
                            fields.push((name, item));
                        }
                    }
                }
                KvData::Struct(fields)
            }
            V::Blob(_) | V::Clob(_) | V::SExp(_) => {
                return Err("holds a value the KV store does not model".into())
            }
        })
    }

    /// "a string", "a number", …: how a refusal names this value's shape.
    pub fn shape(&self) -> &'static str {
        match self {
            KvData::Null => "null",
            KvData::Bool(_) => "a boolean",
            KvData::Number(_) => "a number",
            KvData::String(_) => "a string",
            KvData::List(_) => "a list",
            KvData::Struct(_) => "a struct",
        }
    }

    fn write(&self, out: &mut String) {
        match self {
            KvData::Null => out.push_str("null"),
            KvData::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            KvData::Number(n) => n.write(out),
            KvData::String(s) => crate::runners::write_ion_string(out, s),
            KvData::List(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            KvData::Struct(fields) => {
                out.push('{');
                for (i, (name, item)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    crate::runners::write_ion_symbol(out, name);
                    out.push(':');
                    item.write(out);
                }
                out.push('}');
            }
        }
    }
}

/// What the KV store does not model (the user's ruling, 2026-10-07): a
/// blob, a clob, an s-expression, or an annotation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unmodelled {
    Blob,
    Clob,
    SExp,
    Annotation,
}

impl Unmodelled {
    /// `blob`, `clob`, `sexp`, `annotation`: the name a refusal gives.
    pub fn name(self) -> &'static str {
        match self {
            Unmodelled::Blob => "blob",
            Unmodelled::Clob => "clob",
            Unmodelled::SExp => "sexp",
            Unmodelled::Annotation => "annotation",
        }
    }

    fn with_article(self) -> String {
        match self {
            Unmodelled::Annotation => "an annotation".into(),
            other => format!("a {}", other.name()),
        }
    }
}

/// The first blob, clob or s-expression anywhere in `element` (the type is
/// refused before the annotation rule: w9c `api.put_bodies.annot_blob`);
/// else [`Unmodelled::Annotation`] when any value at any depth is annotated;
/// else `None`. [`decode`] refuses whatever this finds (egret's encoder
/// writes none of them, so the object is not egret's); the API's `PUT`
/// (Task 6) refuses a type with 400 and keeps an annotated body as its raw
/// text (`nested_blob`, `nested_sexp`, `annot_blob`, `nested_annot`). The
/// depth is the reader's, at most `storage::ion::MAX_DEPTH`.
pub fn unmodelled(element: &ion_rs::Element) -> Option<Unmodelled> {
    fn walk(element: &ion_rs::Element, annotated: &mut bool) -> ControlFlow<Unmodelled> {
        use ion_rs::Value as V;
        if !element.annotations().is_empty() {
            *annotated = true;
        }
        match element.value() {
            V::Blob(_) => ControlFlow::Break(Unmodelled::Blob),
            V::Clob(_) => ControlFlow::Break(Unmodelled::Clob),
            V::SExp(_) => ControlFlow::Break(Unmodelled::SExp),
            V::List(seq) => seq.elements().try_for_each(|item| walk(item, annotated)),
            V::Struct(st) => st.fields().try_for_each(|(_, item)| walk(item, annotated)),
            _ => ControlFlow::Continue(()),
        }
    }
    let mut annotated = false;
    match walk(element, &mut annotated) {
        ControlFlow::Break(found) => Some(found),
        ControlFlow::Continue(()) => annotated.then_some(Unmodelled::Annotation),
    }
}

/// A value as egret stores and reads it back: its type and its data.
/// DATETIME, DATE and DURATION are strings in the form w9c
/// `types.<T>.kv_prints` measured (§4.2, §15).
#[derive(Clone, Debug, PartialEq)]
pub struct KvValue {
    pub kind: KvType,
    pub data: KvData,
}

impl KvValue {
    /// A flow's value of `kind` ([`KvData::from_json`]).
    pub fn from_json(kind: KvType, json: &Value) -> Self {
        KvValue {
            kind,
            data: KvData::from_json(json),
        }
    }
}

/// Whether `data` can be a value of `kind` (review M6): what [`decode`]
/// accepts back, so nothing [`encode`] writes is unreadable for its type.
fn agree(kind: KvType, data: &KvData) -> Result<(), String> {
    let ok = match (kind, data) {
        (KvType::String, KvData::String(_) | KvData::Null)
        | (KvType::Number, KvData::Number(_))
        | (KvType::Boolean, KvData::Bool(_))
        | (KvType::Duration, KvData::String(_))
        | (KvType::Json, KvData::List(_) | KvData::Struct(_)) => true,
        (KvType::Datetime, KvData::String(s)) => {
            return instant(s).map(drop).ok_or_else(|| {
                format!("a DATETIME value must be an RFC 3339 instant, and '{s}' is not one")
            })
        }
        (KvType::Date, KvData::String(s)) => {
            return date(s).map(drop).ok_or_else(|| {
                format!("a DATE value must be a YYYY-MM-DD date, and '{s}' is not one")
            })
        }
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(format!("a {kind} value cannot be {}", data.shape()))
    }
}

fn instant(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&chrono::Utc))
}

fn date(s: &str) -> Option<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

/// The object's bytes, byte-equal to w9c `types.<T>.ion_bytes`: Ion text of
/// the data (see [`KvNumber`] for the numbers; a DURATION is an Ion string,
/// `"P1D"` with its quotes), except a DATETIME or DATE, written as the Ion
/// timestamp literal the fixture shows (UTC, millisecond precision, from
/// w9c `types.dt*`: `2025-01-02T03:04:05.000Z`, nanoseconds truncated; a
/// DATE at day precision). Err when the data cannot be a value of its type
/// (review M6: a DURATION holding a number, a DATETIME that is not an
/// instant), naming both.
pub fn encode(v: &KvValue) -> Result<Vec<u8>, String> {
    agree(v.kind, &v.data)?;
    let mut out = String::new();
    match (v.kind, &v.data) {
        (KvType::Datetime, KvData::String(s)) => {
            let t = instant(s).expect("agree checked it");
            out.push_str(&t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string());
        }
        (KvType::Date, KvData::String(s)) => {
            let d = date(s).expect("agree checked it");
            out.push_str(&d.format("%Y-%m-%d").to_string());
        }
        (_, data) => data.write(&mut out),
    }
    Ok(out.into_bytes())
}

/// The write rule (the user's ruling on the value cap, 2026-10-07: anything
/// writable is also readable; review I2): the bytes [`encode`] makes of `v`,
/// refused when they are longer than `cap` (`workflow.storage.max_read_bytes`)
/// or when [`decode`] under the same `cap` would refuse them -- so a stored
/// value always reads, by construction. Every write path (`Set`, `Put`'s
/// merged result, the API's `PUT`) stages exactly these bytes, and a refusal
/// is the author's (413 on the API).
///
/// The bound that bites first for dense JSON is the Ion reader's value
/// budget, one value per 64 bytes of the cap (`storage::ion::ELEMENT_BYTES`:
/// 163 840 values at the 10 MiB default, about 320 KB of small numbers),
/// which bounds the memory a read takes (spec §4.2).
///
/// It runs the Ion reader: call it off the async runtime
/// (`spawn_blocking`), as `store::read_value` runs [`decode`].
pub fn check_readable(v: &KvValue, cap: u64) -> Result<Vec<u8>, String> {
    let bytes = encode(v)?;
    if bytes.len() as u64 > cap {
        return Err(format!(
            "the {} value is {} bytes as stored, more than workflow.storage.max_read_bytes \
             ({cap} bytes) allows a KV value",
            v.kind,
            bytes.len()
        ));
    }
    decode(v.kind, &bytes, cap).map_err(|why| {
        format!(
            "the {} value is not stored: it could not be read back under \
             workflow.storage.max_read_bytes ({cap} bytes): {why}",
            v.kind
        )
    })?;
    Ok(bytes)
}

/// The bytes back to the [`KvData`] of `kind`: exactly one Ion value through
/// `storage::ion::for_each_value` (bounded by `cap`, the preview's reader),
/// with nothing [`unmodelled`] in it at any depth, then normalised for
/// DATETIME/DATE as [`encode`] wrote them and for DURATION as w9c
/// `types._rule_read` prints it. A null inside a map is dropped, as every
/// Kestra read drops it (w9c `types.json_nested`). Err names what is wrong,
/// never a storage key.
///
/// CPU-bound: it runs the Ion reader on its own thread and joins it. Call it
/// off the async runtime, as `store::read_value` does.
pub fn decode(kind: KvType, bytes: &[u8], cap: u64) -> Result<KvData, String> {
    use crate::storage::ion::{for_each_value, Stop};
    let mut values: Vec<Result<KvData, String>> = Vec::with_capacity(1);
    let stop = for_each_value(bytes, cap, |element| {
        if !values.is_empty() {
            return ControlFlow::Break(());
        }
        values.push(KvData::from_element(element, cap));
        ControlFlow::Continue(())
    });
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
    let data = match values.pop() {
        None => return Err(format!("the stored {kind} value holds no Ion value")),
        Some(Err(why)) => {
            return Err(format!(
                "the stored {kind} value {why}; egret's KV store writes none, so the object \
                 was not written by it"
            ))
        }
        Some(Ok(data)) => data,
    };
    match (kind, data) {
        // A rendered `null` is stored as Ion `null`, typed STRING (`KvType::of`).
        (KvType::String, d @ (KvData::String(_) | KvData::Null)) => Ok(d),
        (KvType::Number, d @ KvData::Number(_)) => Ok(d),
        (KvType::Boolean, d @ KvData::Bool(_)) => Ok(d),
        (KvType::Json, d @ (KvData::Struct(_) | KvData::List(_))) => Ok(d),
        (KvType::Datetime, KvData::String(s)) => instant(&s)
            .map(|t| KvData::String(java_instant(t)))
            .ok_or_else(|| format!("the stored DATETIME value '{s}' is not an instant")),
        (KvType::Date, KvData::String(s)) => date(&s)
            .map(|d| KvData::String(d.format("%Y-%m-%d").to_string()))
            .ok_or_else(|| format!("the stored DATE value '{s}' is not a date")),
        (KvType::Duration, KvData::String(s)) => {
            Ok(KvData::String(kestra_duration(&s).unwrap_or(s)))
        }
        (_, d) => Err(format!(
            "the stored {kind} value is {}, not a {kind}",
            d.shape()
        )),
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
    use serde_json::json;

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
            let data = match kind {
                // A Double (Jackson's read of a non-integer number).
                KvType::Number
                    if !cell["ion_bytes"]
                        .as_str()
                        .unwrap()
                        .bytes()
                        .all(|b| b.is_ascii_digit() || b == b'-') =>
                {
                    KvData::Number(KvNumber::Float(get.as_f64().unwrap()))
                }
                // A JSON value: the set input as Jackson parses it (keeps
                // the nulls inside maps the read drops).
                KvType::Json => {
                    let input = cell["set"]["value"].as_str().unwrap();
                    KvData::from_json(&serde_json::from_str(input).unwrap())
                }
                // A DURATION is stored as written.
                KvType::Duration => KvData::from_json(&cell["set"]["value"]),
                _ => KvData::from_json(&get),
            };
            rows.push((
                name,
                KvValue { kind, data },
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
            assert_eq!(
                String::from_utf8(encode(v).unwrap()).unwrap(),
                *bytes,
                "{name}"
            );
            kinds.insert(v.kind);
        }
        assert_eq!(kinds.len(), 7, "every type is covered: {kinds:?}");
    }

    #[test]
    fn decode_reads_back_what_encode_wrote_for_every_type() {
        for (name, v, _, get) in types_rows() {
            let back = decode(v.kind, &encode(&v).unwrap(), 1 << 20)
                .unwrap()
                .to_json();
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
            let got = decode(kind, bytes.as_bytes(), 1 << 20).unwrap().to_json();
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
            data: KvData::String("P1D".into()),
        };
        assert_eq!(encode(&v).unwrap(), b"\"P1D\"");
        assert_eq!(
            decode(KvType::Duration, b"\"P1D\"", 4096).unwrap(),
            KvData::String("PT24H".into())
        );
        // Not Kestra's pattern: as written (w9c set_coercion.dur_lower).
        for raw in ["p1d", "-PT1S", "PT1.5S", "PD", "PT", "P", "P1DT", "P1H"] {
            let bytes = encode(&KvValue {
                kind: KvType::Duration,
                data: KvData::String(raw.into()),
            })
            .unwrap();
            assert_eq!(
                decode(KvType::Duration, &bytes, 4096).unwrap(),
                KvData::String(raw.into()),
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

    fn dec(plain: &str) -> KvData {
        KvData::Number(KvNumber::decimal(plain).unwrap())
    }

    fn written(kind: KvType, data: KvData) -> String {
        String::from_utf8(encode(&KvValue { kind, data }).unwrap()).unwrap()
    }

    /// An API body as Task 6 will take it: one Ion value, through the same
    /// walk and conversion `decode` uses.
    fn api_body(body: &str) -> KvData {
        let element = ion_rs::Element::read_one(body.as_bytes()).unwrap();
        KvData::from_element(&element, 1 << 20).unwrap()
    }

    #[test]
    fn the_api_decimals_and_non_finite_numbers_are_byte_equal_to_the_artifact() {
        // w9c api.put_bodies: ion_float `1.5`, dec_150 `1.50` -> `1.5`,
        // dec_0_1 `0.1`, ion_decimal_d `1.5d0` -> `1.5`, ion_nan.
        for (data, bytes) in [
            (dec("1.5"), "1.5"),
            (dec("1.50"), "1.5"),
            (dec("0.1"), "0.1"),
            (dec("-0.025"), "-0.025"),
            (dec("2"), "2."),
            (dec("200.00"), "200."),
            (dec("-0.0"), "0."),
            (
                dec("123456789012345678901234567890.5"),
                "123456789012345678901234567890.5",
            ),
        ] {
            assert_eq!(written(KvType::Number, data), bytes);
        }
        for bad in ["1e5", "", "-", ".5", "5.", "1.5d0", "a", "1.5e0", "--1"] {
            assert!(KvNumber::decimal(bad).is_none(), "{bad}");
        }
        // The controls: Doubles of the same values stay Ion floats.
        for (json, bytes) in [
            (json!(1.5), "1.5e0"),
            (json!(1.5e20), "1.5e20"),
            (json!(1e-7), "1.0e-7"),
        ] {
            let v = KvValue::from_json(KvType::Number, &json);
            assert_eq!(String::from_utf8(encode(&v).unwrap()).unwrap(), bytes);
        }
        let nan = KvData::Number(KvNumber::Float(f64::NAN));
        assert_eq!(written(KvType::Number, nan), "nan");
        let back = decode(KvType::Number, b"nan", 4096).unwrap();
        assert_eq!(back.to_json(), "NaN");
        assert_eq!(written(KvType::Number, back), "nan");
        assert_eq!(
            decode(KvType::Number, b"1.5", 4096).unwrap(),
            dec("1.5"),
            "an Ion decimal reads back as a decimal"
        );
    }

    /// The user's ruling (2026-10-07): an API-style Ion `{a:1.5}` is
    /// encoded, decoded and re-encoded without change, and the decimal is
    /// still a decimal at every step.
    #[test]
    fn an_api_decimal_survives_encode_decode_encode_unchanged() {
        let body = api_body("{a:1.5}");
        assert_eq!(
            body,
            KvData::Struct(vec![("a".into(), dec("1.5"))]),
            "the body's 1.5 is a decimal"
        );
        let first = written(KvType::Json, body);
        assert_eq!(first, "{a:1.5}");
        let read = decode(KvType::Json, first.as_bytes(), 4096).unwrap();
        assert_eq!(read, KvData::Struct(vec![("a".into(), dec("1.5"))]));
        let second = written(KvType::Json, read.clone());
        assert_eq!(second, first, "re-encoded byte for byte");
        assert_eq!(read.to_json(), json!({"a": 1.5}));
    }

    /// No digit is lost: neither through a read nor through a write.
    #[test]
    fn a_decimal_of_thirty_one_digits_is_exact() {
        const BIG: &str = "123456789012345678901234567890.5";
        let body = api_body(BIG);
        assert_eq!(body, dec(BIG));
        let bytes = written(KvType::Number, body);
        assert_eq!(bytes, BIG);
        let read = decode(KvType::Number, bytes.as_bytes(), 4096).unwrap();
        assert_eq!(read, dec(BIG));
        assert_eq!(
            read.to_json().to_string(),
            BIG,
            "the reader sees every digit"
        );
        assert_eq!(written(KvType::Number, read), BIG);
        // A float of the same text is a Double: what a flow makes of it.
        let float = KvData::from_json(&serde_json::from_str(BIG).unwrap());
        assert_eq!(
            float,
            KvData::Number(KvNumber::Float(1.2345678901234568e29))
        );
        // Exponent and scale come through as plain digits.
        for (ion, plain) in [
            ("1.5d0", "1.5"),
            ("15d-1", "1.5"),
            ("1d3", "1000"),
            ("1.50", "1.50"),
            ("0.0000001", "0.0000001"),
            ("-12345678d-10", "-0.0012345678"),
            ("-0.0", "0.0"),
            ("0d5", "0"),
        ] {
            assert_eq!(api_body(ion), dec(plain), "{ion}");
        }
        // Integers stay integers at any size.
        let int = "-123456789012345678901234567890";
        assert_eq!(api_body(int), KvData::Number(KvNumber::Int(int.into())));
        assert_eq!(
            KvData::from_json(&serde_json::from_str(int).unwrap()),
            KvData::Number(KvNumber::Int(int.into()))
        );
        assert_eq!(written(KvType::Number, api_body(int)), int);
    }

    /// A decimal whose plain digits would not fit the cap is refused, not
    /// expanded: eleven bytes of Ion must not become 100 MB of text.
    #[test]
    fn a_decimal_with_a_huge_exponent_is_refused_not_expanded() {
        for ion in ["1d99999999", "1d-99999999", "[1d9999]"] {
            let err = decode(KvType::Json, ion.as_bytes(), 4096)
                .or_else(|_| decode(KvType::Number, ion.as_bytes(), 4096))
                .unwrap_err();
            assert!(err.contains("max_read_bytes"), "{ion}: {err}");
        }
        assert_eq!(
            decode(KvType::Number, b"1d9", 4096).unwrap(),
            dec("1000000000")
        );
    }

    /// Review NI1: the expansion budget is the WHOLE value's, not each
    /// decimal's. At the 10 MiB cap two decimals of nine million digits each
    /// (18 MB of text from 23 bytes of Ion) are refused; two of four million
    /// (8 MB) read.
    #[test]
    fn the_decoded_text_of_a_whole_value_is_bounded_by_the_cap() {
        const CAP: u64 = 10 << 20;
        let err = decode(KvType::Json, b"[1d9000000,1d9000000]", CAP).unwrap_err();
        assert_eq!(
            err,
            "the stored JSON value decodes to more than workflow.storage.max_read_bytes \
             (10485760 bytes) of digits and text; egret's KV store writes none, so the object \
             was not written by it"
        );
        let element = ion_rs::Element::read_one(&b"[1d9000000,1d9000000]"[..]).unwrap();
        assert!(
            KvData::from_element(&element, CAP).is_err(),
            "the API path too"
        );
        let KvData::List(items) = decode(KvType::Json, b"[1d4000000,1d4000000]", CAP).unwrap()
        else {
            panic!()
        };
        assert_eq!(items.len(), 2);
        assert!(matches!(&items[1], KvData::Number(KvNumber::Decimal(t)) if t.len() == 4_000_001));
        // Strings and field names share the same budget: 3 x 2 000 bytes of
        // text under a 5 000-byte budget is refused, 2 x 2 000 reads.
        let s = "x".repeat(2000);
        let three = format!("[\"{s}\",\"{s}\",{{'{s}':1}}]");
        let two = format!("[\"{s}\",{{'{s}':1}}]");
        assert!(
            KvData::from_element(&ion_rs::Element::read_one(three.as_bytes()).unwrap(), 5000)
                .is_err()
        );
        assert!(
            KvData::from_element(&ion_rs::Element::read_one(two.as_bytes()).unwrap(), 5000).is_ok()
        );
        // A zero with a huge exponent is one digit, not refused.
        assert_eq!(
            decode(KvType::Number, b"0d99999999", 4096).unwrap(),
            dec("0")
        );
    }

    /// Review NM1: a leading zero is not a number Ion or JSON reads.
    #[test]
    fn a_leading_zero_is_not_an_int_or_a_decimal() {
        for bad in ["007", "-007", "00", "01"] {
            assert!(KvNumber::int(bad).is_none(), "{bad}");
        }
        for bad in ["00.5", "-00.5", "007", "01.0"] {
            assert!(KvNumber::decimal(bad).is_none(), "{bad}");
        }
        for good in ["0", "-0", "10", "-7"] {
            assert!(KvNumber::int(good).is_some(), "{good}");
        }
        for good in ["0.5", "-0.05", "10.0", "0", "100"] {
            assert!(KvNumber::decimal(good).is_some(), "{good}");
        }
    }

    /// Every API body of the fixture that is a number or JSON: the stored
    /// bytes are the cell's, and the reader sees the cell's value
    /// (`nested_dec` among them).
    #[test]
    fn the_api_number_and_json_cells_hold() {
        let f = fixture();
        let bodies = f["egret_expected"]["api"]["put_bodies"]
            .as_object()
            .unwrap();
        let mut examined = vec![];
        for (name, cell) in bodies {
            if cell["put_status"] != 200 {
                continue;
            }
            let kind = KvType::parse(cell["get"]["type"].as_str().unwrap()).unwrap();
            let body = cell["body"].as_str().unwrap();
            let Ok(element) = ion_rs::Element::read_one(body.trim().as_bytes()) else {
                continue; // raw text: Task 6
            };
            let data = match (kind, KvData::from_element(&element, 1 << 20)) {
                (KvType::Number | KvType::Json, Ok(d)) => d,
                _ => continue,
            };
            let bytes = written(kind, data);
            assert_eq!(bytes, cell["ion_bytes"].as_str().unwrap(), "{name}");
            let read = decode(kind, bytes.as_bytes(), 4096).unwrap();
            assert_eq!(read.to_json(), cell["get"]["value"], "{name}");
            assert_eq!(written(kind, read), bytes, "{name}: stable");
            examined.push(name.as_str());
        }
        for must in [
            "nested_dec",
            "ion_float",
            "dec_150",
            "dec_0_1",
            "ion_decimal_d",
            "ion_float_e",
            "float_1e2",
            "ion_big",
            "ion_nan",
            "ion_struct",
        ] {
            assert!(examined.contains(&must), "{must} examined: {examined:?}");
        }
    }

    /// What Task 3's `Put` does to a stored value: decode, change the
    /// merge-ready value, encode. The decimals already there stay decimals.
    #[test]
    fn a_merge_style_round_trip_keeps_the_decimal() {
        let stored = b"{a:1.5,c:{d:123456789012345678901234567890.5}}";
        let mut data = decode(KvType::Json, stored, 4096).unwrap();
        let KvData::Struct(fields) = &mut data else {
            panic!("{data:?}")
        };
        // A flow's patch `{b: 2, e: 0.5}`: an int and a float.
        let patch = KvData::from_json(&json!({"b": 2, "e": 0.5}));
        let KvData::Struct(patch) = patch else {
            unreachable!()
        };
        fields.extend(patch);
        assert_eq!(
            written(KvType::Json, data),
            "{a:1.5,c:{d:123456789012345678901234567890.5},b:2,e:0.5e0}"
        );
    }

    #[test]
    fn floats_stay_floats_and_ints_stay_ints() {
        let v = KvValue::from_json(KvType::Json, &json!({"f": 1.5, "i": 2, "g": 100.0}));
        let bytes = String::from_utf8(encode(&v).unwrap()).unwrap();
        assert_eq!(bytes, "{f:1.5e0,i:2,g:100e0}");
        let read = decode(KvType::Json, bytes.as_bytes(), 4096).unwrap();
        assert_eq!(
            read,
            KvData::Struct(vec![
                ("f".into(), KvData::Number(KvNumber::Float(1.5))),
                ("i".into(), KvData::Number(KvNumber::Int("2".into()))),
                ("g".into(), KvData::Number(KvNumber::Float(100.0))),
            ])
        );
        assert_eq!(read.to_json().to_string(), r#"{"f":1.5,"i":2,"g":100.0}"#);
    }

    /// Review M1: a blob, clob, s-expression or annotation at any depth is
    /// refused naming it (egret writes none), never read as another type.
    #[test]
    fn decode_refuses_what_the_kv_store_does_not_model_at_any_depth() {
        for (kind, bytes, name) in [
            (KvType::String, "{{aGVsbG8=}}", "a blob"),
            (KvType::String, "{{\"hello\"}}", "a clob"),
            (KvType::Json, "(a b)", "a sexp"),
            (KvType::Json, "{a:{{aGVsbG8=}}}", "a blob"),
            (KvType::Json, "[1,[2,(b)]]", "a sexp"),
            (KvType::Number, "x::1", "an annotation"),
            (KvType::Json, "{a:[1,x::2]}", "an annotation"),
            (KvType::Json, "x::{a:{{\"c\"}}}", "a clob"),
        ] {
            let err = decode(kind, bytes.as_bytes(), 4096).unwrap_err();
            assert!(
                err.contains(&format!("holds {name}, which the KV store does not model")),
                "{bytes}: {err}"
            );
        }
        // The walk Task 6 reuses: a type before an annotation.
        let walk = |body: &str| unmodelled(&ion_rs::Element::read_one(body).unwrap());
        assert_eq!(walk("x::{{aGVsbG8=}}"), Some(Unmodelled::Blob));
        assert_eq!(walk("{a:x::1,b:(c)}"), Some(Unmodelled::SExp));
        assert_eq!(walk("{a:x::1}"), Some(Unmodelled::Annotation));
        assert_eq!(walk("{a:[1,{b:\"c\"}]}"), None);
        assert_eq!(Unmodelled::Clob.name(), "clob");
    }

    /// Review M6: `encode` refuses data that is not of its type, so a
    /// DURATION holding a number or a DATETIME that is not an instant is
    /// never written.
    #[test]
    fn encode_refuses_data_that_is_not_of_its_type() {
        let num = KvData::Number(KvNumber::Int("1".into()));
        let s = |t: &str| KvData::String(t.into());
        for (kind, data, says) in [
            (
                KvType::Duration,
                num.clone(),
                "a DURATION value cannot be a number",
            ),
            (KvType::Datetime, s("not a date"), "'not a date' is not one"),
            (
                KvType::Datetime,
                num.clone(),
                "a DATETIME value cannot be a number",
            ),
            (KvType::Date, s("2025-13-01"), "'2025-13-01' is not one"),
            (KvType::Json, s("{}"), "a JSON value cannot be a string"),
            (KvType::Json, KvData::Null, "a JSON value cannot be null"),
            (
                KvType::Boolean,
                s("true"),
                "a BOOLEAN value cannot be a string",
            ),
            (KvType::Number, s("1"), "a NUMBER value cannot be a string"),
            (
                KvType::String,
                num.clone(),
                "a STRING value cannot be a number",
            ),
        ] {
            let err = encode(&KvValue { kind, data }).unwrap_err();
            assert!(err.contains(says), "{kind}: {err}");
        }
        // The controls: each type with data of its own reads back.
        for (kind, data) in [
            (KvType::String, s("x")),
            (KvType::String, KvData::Null),
            (KvType::Number, num),
            (KvType::Boolean, KvData::Bool(true)),
            (KvType::Datetime, s("2025-01-02T03:04:05+07:00")),
            (KvType::Date, s("2025-01-02")),
            (KvType::Duration, s("p1d")),
            (KvType::Json, KvData::List(vec![])),
        ] {
            let v = KvValue { kind, data };
            check_readable(&v, 4096).unwrap_or_else(|e| panic!("{kind}: {e}"));
        }
    }

    /// Review I2: anything writable is readable. The reviewer's case, a
    /// 400 KB list of small numbers under the 10 MiB default, is over the
    /// reader's value budget, so it is refused at write with the reason --
    /// never written and then unreadable. Under the budget it reads.
    #[test]
    fn check_readable_refuses_at_write_what_decode_would_refuse() {
        const CAP: u64 = 10 << 20;
        let zeros = |n: usize| KvValue::from_json(KvType::Json, &Value::Array(vec![json!(0); n]));
        let big = zeros(200_000);
        let bytes = encode(&big).unwrap();
        assert_eq!(bytes.len(), 400_001, "400 KB, far under the cap");
        let err = check_readable(&big, CAP).unwrap_err();
        assert!(
            err.starts_with(
                "the JSON value is not stored: it could not be read back under \
                 workflow.storage.max_read_bytes (10485760 bytes)"
            ) && err.contains("more than 163840 values"),
            "{err}"
        );
        // The control: under the budget, written and read.
        let fits = zeros(150_000);
        let bytes = check_readable(&fits, CAP).unwrap();
        let KvData::List(items) = decode(KvType::Json, &bytes, CAP).unwrap() else {
            panic!()
        };
        assert_eq!(items.len(), 150_000);
        // Over the cap in bytes: refused naming it, before any decode.
        let long = KvValue::from_json(KvType::String, &json!("x".repeat(5000)));
        let err = check_readable(&long, 4096).unwrap_err();
        assert_eq!(
            err,
            "the STRING value is 5002 bytes as stored, more than \
             workflow.storage.max_read_bytes (4096 bytes) allows a KV value"
        );
        // A refusal of the type comes first.
        let bad = KvValue {
            kind: KvType::Duration,
            data: KvData::Bool(true),
        };
        assert!(check_readable(&bad, CAP).unwrap_err().contains("DURATION"));
    }

    /// Review I2 / M3: the system symbol table is no longer charged, so a
    /// value with no symbol of its own reads at a cap of a few bytes; the
    /// configured floor (`max_read_bytes` >= 4 KiB) reads every small value.
    #[test]
    fn a_small_value_reads_under_a_small_cap() {
        assert_eq!(
            decode(KvType::Duration, b"\"P1D\"", 64).unwrap(),
            KvData::String("PT24H".into())
        );
        assert_eq!(
            decode(KvType::Boolean, b"true", 256).unwrap(),
            KvData::Bool(true)
        );
        assert!(decode(KvType::Json, b"{a:1,b:[2,3]}", 4096).is_ok());
    }

    #[test]
    fn the_type_of_a_value_and_the_kvtype_spelling() {
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
        // of_data names a KvData as `of` names its JSON.
        for v in [
            json!("s"),
            json!(null),
            json!(1),
            json!(1.5),
            json!(true),
            json!({}),
            json!([]),
        ] {
            assert_eq!(
                KvType::of_data(&KvData::from_json(&v)),
                KvType::of(&v),
                "{v}"
            );
        }
        assert_eq!(KvType::of_data(&dec("1.5")), KvType::Number);
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
