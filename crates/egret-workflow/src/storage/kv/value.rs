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

use crate::storage::ion::Stop;

/// The most digits a number in a KV value has (the user's ruling,
/// 2026-10-07): an int's digits, a decimal's coefficient digits as written
/// (both sides of the point, in plain notation as egret writes it, so
/// `1d5000` written out is 5,001), a float's mantissa digits as written,
/// and an exponent's digits on their own. Enforced before `ion-rs` parses
/// a number ([`for_each_kv_value`]) and on a flow's numbers
/// ([`KvData::from_json`]); a longer number is refused, never truncated.
pub const MAX_NUMBER_DIGITS: u64 = 1000;

/// Why a value cannot be stored or read back (review P2): the variant is the
/// class, the text the message, so a caller maps the variant and never
/// reads the text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unstorable {
    /// Over a bound: the cap (`workflow.storage.max_read_bytes`) in bytes,
    /// the dense-value limit, the decoded-text budget, a symbol table over
    /// the cap, or a number over [`MAX_NUMBER_DIGITS`]. `NsError::TooLarge`
    /// (413 on the API).
    TooLarge(String),
    /// Anything else: data not of its type, not one Ion value, nesting past
    /// `storage::ion::MAX_DEPTH`, a type the KV store does not model, a
    /// reader that crashed. `NsError::Author` (400 on the API).
    Invalid(String),
}

impl Unstorable {
    /// The message, whatever the class.
    pub fn message(&self) -> &str {
        match self {
            Unstorable::TooLarge(m) | Unstorable::Invalid(m) => m,
        }
    }

    /// The same class, its message rewritten by `f`.
    fn map(self, f: impl FnOnce(String) -> String) -> Self {
        match self {
            Unstorable::TooLarge(m) => Unstorable::TooLarge(f(m)),
            Unstorable::Invalid(m) => Unstorable::Invalid(f(m)),
        }
    }
}

impl std::fmt::Display for Unstorable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

/// THE way the KV store reads Ion: `storage::ion::for_each_value_limited`
/// under `cap` (`workflow.storage.max_read_bytes`) with the digit limit
/// [`MAX_NUMBER_DIGITS`], so a long number is refused before `ion-rs`
/// parses it (re-review 2, N1: `ion-rs` truncates a decimal past 65,535
/// digits silently; N2: it parses a long number in quadratic time).
/// [`decode`] (hence [`check_readable`] and every write) reads through it,
/// and so must the API's `PUT` body (Task 6) -- never
/// `ion_rs::Element::read_one`, which has no depth guard, no value budget,
/// no symbol charge and no digit limit.
pub(crate) fn for_each_kv_value<F>(bytes: &[u8], cap: u64, sink: F) -> Stop
where
    F: FnMut(&ion_rs::Element) -> ControlFlow<()> + Send,
{
    crate::storage::ion::for_each_value_limited(bytes, cap, Some(MAX_NUMBER_DIGITS), sink)
}

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
    /// A negative zero is zero (W9c Task 5: w9c `types.num_neg0`, `-0` is
    /// stored `0`, as Jackson reads it into an `Integer`).
    pub fn int(text: &str) -> Option<Self> {
        if text == "-0" {
            return Some(KvNumber::Int("0".into()));
        }
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
    fn of_ion_decimal(d: &ion_rs::Decimal, budget: &mut Budget) -> Result<Self, Unstorable> {
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
        // Exactly the length of the text built below (re-review 2's nit:
        // one byte over it refused a value egret wrote at the cap).
        let sign_len = sign.len() as u64;
        let plain_len = if d.is_zero() && exponent >= 0 {
            1
        } else if exponent >= 0 {
            sign_len + len.saturating_add(shift)
        } else if len > shift {
            sign_len + len + 1
        } else {
            sign_len + shift.saturating_add(2)
        };
        // The digit limit on READ too (re-review 3, M1): the scan counts the
        // digits as written, and `1d999999` (8 bytes) or a 432-byte binary
        // coefficient expand past it here. Refused before the text is built.
        within_digit_limit(plain_len - sign_len - u64::from(exponent < 0))?;
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

    fn charge(&mut self, bytes: u64) -> Result<(), Unstorable> {
        match self.left.checked_sub(bytes) {
            Some(left) => {
                self.left = left;
                Ok(())
            }
            None => Err(Unstorable::TooLarge(format!(
                "decodes to more than workflow.storage.max_read_bytes ({} bytes) of digits and \
                 text",
                self.cap
            ))),
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

/// Dismantles the tree with an explicit stack (Task 3 re-review 2, H1): a
/// derived drop recurses once per level, and a value nested ~5,000 deep
/// overflowed a 2 MiB stack and aborted the process. Each container's
/// children are moved onto a heap stack before it is freed, so a drop
/// takes O(1) stack at any depth.
impl Drop for KvData {
    fn drop(&mut self) {
        fn take_children(d: &mut KvData, stack: &mut Vec<KvData>) {
            match d {
                KvData::List(items) => stack.append(items),
                KvData::Struct(fields) => {
                    stack.extend(std::mem::take(fields).into_iter().map(|(_, v)| v))
                }
                _ => {}
            }
        }
        if !matches!(self, KvData::List(_) | KvData::Struct(_)) {
            return;
        }
        let mut stack = Vec::new();
        take_children(self, &mut stack);
        while let Some(mut child) = stack.pop() {
            take_children(&mut child, &mut stack);
            // `child` drops here, childless.
        }
    }
}

/// The deepest nesting of containers a KV value may hold:
/// [`crate::storage::ion::MAX_DEPTH`], which reads enforce.
pub(crate) fn too_deep() -> Unstorable {
    Unstorable::Invalid(format!(
        "the value nests containers deeper than {} levels, which egret does not store",
        crate::storage::ion::MAX_DEPTH
    ))
}

/// Whether `v` nests containers deeper than `limit`, walked with an
/// explicit stack: O(1) call stack at any depth (H1).
fn json_deeper_than(v: &Value, limit: usize) -> bool {
    let mut stack: Vec<(&Value, usize)> = vec![(v, 0)];
    while let Some((v, depth)) = stack.pop() {
        let children: Box<dyn Iterator<Item = &Value>> = match v {
            Value::Array(items) => Box::new(items.iter()),
            Value::Object(map) => Box::new(map.values()),
            _ => continue,
        };
        if depth + 1 > limit {
            return true;
        }
        stack.extend(children.map(|c| (c, depth + 1)));
    }
    false
}

impl KvData {
    /// Whether this value nests containers deeper than `limit`, walked with
    /// an explicit stack (H1): the store's writers ask it before they clone
    /// or encode, so a value built by hand cannot reach a recursive path.
    pub fn nests_deeper_than(&self, limit: usize) -> bool {
        let mut stack: Vec<(&KvData, usize)> = vec![(self, 0)];
        while let Some((d, depth)) = stack.pop() {
            let children: Box<dyn Iterator<Item = &KvData>> = match d {
                KvData::List(items) => Box::new(items.iter()),
                KvData::Struct(fields) => Box::new(fields.iter().map(|(_, v)| v)),
                _ => continue,
            };
            if depth + 1 > limit {
                return true;
            }
            stack.extend(children.map(|c| (c, depth + 1)));
        }
        false
    }

    /// A flow's value, with `serde_json`'s semantics (the user's ruling,
    /// 2026-10-07): a number written with only digits is an int, any other
    /// (a fraction, an exponent) a float, as Kestra's Jackson reads a JSON
    /// or rendered number. A flow never makes a decimal. A null inside an
    /// object is kept (Kestra writes it; every read drops it).
    ///
    /// A number of more than [`MAX_NUMBER_DIGITS`] digits (its mantissa as
    /// written, or its exponent) is refused, `TooLarge`, before it becomes a
    /// float that would drop its digits silently (the user's ruling,
    /// 2026-10-07).
    ///
    /// A value nesting containers deeper than
    /// [`crate::storage::ion::MAX_DEPTH`] is refused, `Invalid`, before
    /// anything is built (Task 3 re-review 2, H1): the check walks with an
    /// explicit stack, so no deeper value is ever built from JSON, and the
    /// conversion's own recursion is bounded by it.
    pub fn from_json(v: &Value) -> Result<Self, Unstorable> {
        if json_deeper_than(v, crate::storage::ion::MAX_DEPTH) {
            return Err(too_deep());
        }
        Self::from_json_bounded(v)
    }

    /// [`KvData::from_json`] once the depth is checked.
    fn from_json_bounded(v: &Value) -> Result<Self, Unstorable> {
        Ok(match v {
            Value::Null => KvData::Null,
            Value::Bool(b) => KvData::Bool(*b),
            Value::Number(n) => {
                let text = n.to_string();
                check_json_digits(&text)?;
                KvData::Number(KvNumber::int(&text).unwrap_or_else(|| {
                    // A text `serde_json` accepted but `f64` does not parse
                    // cannot occur; out of range is an infinity, as in Java.
                    KvNumber::Float(text.parse().unwrap_or(f64::NAN))
                }))
            }
            Value::String(s) => KvData::String(s.clone()),
            Value::Array(items) => KvData::List(
                items
                    .iter()
                    .map(Self::from_json_bounded)
                    .collect::<Result<_, _>>()?,
            ),
            Value::Object(map) => KvData::Struct(
                map.iter()
                    .map(|(name, item)| Ok((name.clone(), Self::from_json_bounded(item)?)))
                    .collect::<Result<_, Unstorable>>()?,
            ),
        })
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
    ///
    /// `element` must come from [`for_each_kv_value`], whose digit limit
    /// this conversion relies on (an `Element` from anywhere else may hold a
    /// number `ion-rs` already truncated). Err is `Invalid` for what the KV
    /// store does not model and `TooLarge` for the budget.
    pub fn from_element(element: &ion_rs::Element, cap: u64) -> Result<Self, Unstorable> {
        if let Some(found) = unmodelled(element) {
            return Err(Unstorable::Invalid(format!(
                "holds {}, which the KV store does not model",
                found.with_article()
            )));
        }
        Self::convert(element, &mut Budget::new(cap))
    }

    /// `element` as KvData, every copied text charged to `budget` (shared by
    /// the whole value, never per leaf).
    fn convert(element: &ion_rs::Element, budget: &mut Budget) -> Result<Self, Unstorable> {
        use ion_rs::Value as V;
        let text = |t: &str, budget: &mut Budget| -> Result<String, Unstorable> {
            budget.charge(t.len() as u64)?;
            Ok(t.to_string())
        };
        Ok(match element.value() {
            V::Null(_) => KvData::Null,
            V::Bool(b) => KvData::Bool(*b),
            // An int's digits are at most a few times the bytes that hold it
            // in the element already; charged once built.
            // The digit limit on read (re-review 3, M1): `0x` + 1,000 hex
            // digits is 1,205 decimal digits. The text is bounded by the
            // scan (at most 1,000 written digits or 432 binary bytes), so it
            // is built, then measured, then refused before it is charged.
            V::Int(i) => {
                let digits = i.to_string();
                within_digit_limit(digits.trim_start_matches('-').len() as u64)?;
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
                return Err(Unstorable::Invalid(
                    "holds a value the KV store does not model".into(),
                ))
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
    /// A flow's value of `kind` ([`KvData::from_json`], whose only refusal
    /// is a number over [`MAX_NUMBER_DIGITS`], `TooLarge`).
    pub fn from_json(kind: KvType, json: &Value) -> Result<Self, Unstorable> {
        Ok(KvValue {
            kind,
            data: KvData::from_json(json)?,
        })
    }
}

/// Why an API `PUT` body cannot be stored (W9c spec §9; Task 6). Never the
/// raw-text case: [`Unmodelled::Annotation`] is answered as the raw text by
/// [`parse_api_body`] and never reaches [`ApiBodyError::Unmodelled`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiBodyError {
    /// No Ion value at all: a 0-byte body, or only whitespace (the user's
    /// ruling R-b, 2026-10-07). 400 "empty body".
    Empty,
    /// The raw-text fallback, whose bytes are not UTF-8 (re-review 3, M3):
    /// such a body cannot be a STRING. 400 "not UTF-8 text".
    NotUtf8,
    /// A blob, a clob or an s-expression, at any depth (never `Annotation`).
    /// 400 naming the type.
    Unmodelled(Unmodelled),
    /// The reader's byte cap, the dense-value limit, the symbol charge, the
    /// 1,000-digit limit, or `from_element`'s text budget. 413.
    TooLarge(String),
    /// Nesting past `storage::ion::MAX_DEPTH`. 400.
    TooDeep(String),
    /// The reader's thread panicked on the body (logged at WARN). 400.
    Crashed(String),
}

impl ApiBodyError {
    /// The refusal's text, as the API sends it.
    pub fn message(&self) -> String {
        match self {
            ApiBodyError::Empty => {
                "empty body: a KV value is one Ion value, or any text, and the body holds \
                 neither"
                    .into()
            }
            ApiBodyError::NotUtf8 => {
                "the body is not one Ion value, and it is not UTF-8 text either, so it cannot \
                 be stored as a STRING: not UTF-8 text"
                    .into()
            }
            ApiBodyError::Unmodelled(found) => format!(
                "the body holds {}, a type the KV store does not model; nothing was stored",
                found.with_article()
            ),
            ApiBodyError::TooLarge(m) => format!("the body cannot be stored: {m}"),
            ApiBodyError::TooDeep(m) => format!("the body cannot be stored: {m}"),
            ApiBodyError::Crashed(m) => format!("the body could not be read: {m}"),
        }
    }
}

/// The PUT body (`KVController.java:124-141`): parsed as ONE Ion value
/// through [`for_each_kv_value`] (never `Element::read_one`), its type as
/// `KVType.from` infers it (w9c `api.put_bodies`).
///
/// How reading stopped decides first:
/// - not Ion (`Stop::Failed`), or more than one value (`Stop::Full`): the
///   raw text, typed STRING ([`ApiBodyError::NotUtf8`] when the bytes are
///   not UTF-8); no value at all: [`ApiBodyError::Empty`];
/// - a refusal of the reader (`TooLarge`, `TooDeep`, `Crashed`) is that
///   error, never the raw text (fix round 3, re-review P1).
///
/// Then the one value (the user's rulings, 2026-10-07, spec §9):
/// - a blob, clob or sexp at any depth: [`ApiBodyError::Unmodelled`]
///   naming it, before the annotation rule (`x::{{aGVsbG8=}}` names
///   `blob`: re-review N3);
/// - an annotation at any depth: the whole body is the raw text, STRING;
/// - a timestamp with a time: a DATETIME, in UTC, even at midnight; a
///   day-precision one: a DATE; a year- or month-precision one (R-c): the
///   raw text, STRING;
/// - a string or a symbol: a DURATION when its text matches Kestra's
///   DURATION_PATTERN (w9c `types._rule_read`; `PT1H`, `"P1D"`), kept as
///   written; else a STRING;
/// - anything else: [`KvData::from_element`] under `cap` (one budget for
///   the whole value: fix round 2, NI1), typed by [`KvType::of_data`] (an
///   int, decimal or float NUMBER, a list or struct JSON, a null STRING).
///
/// `cap` is `workflow.storage.max_read_bytes`. CPU-bound: call it off the
/// async runtime.
pub fn parse_api_body(body: &[u8], cap: u64) -> Result<KvValue, ApiBodyError> {
    let mut found: Vec<ion_rs::Element> = Vec::with_capacity(1);
    let stop = for_each_kv_value(body, cap, |element| {
        if !found.is_empty() {
            return ControlFlow::Break(());
        }
        found.push(element.clone());
        ControlFlow::Continue(())
    });
    let element = match stop {
        Stop::End => match found.pop() {
            Some(element) => element,
            None => return Err(ApiBodyError::Empty),
        },
        Stop::Failed(_) | Stop::Full => return raw_text(body),
        Stop::TooLarge(m) => return Err(ApiBodyError::TooLarge(m)),
        Stop::TooDeep(m) => return Err(ApiBodyError::TooDeep(m)),
        Stop::Crashed(m) => {
            tracing::warn!(error = %m, "the Ion reader crashed on a KV API body");
            return Err(ApiBodyError::Crashed(m));
        }
    };
    match unmodelled(&element) {
        Some(Unmodelled::Annotation) => return raw_text(body),
        Some(found) => return Err(ApiBodyError::Unmodelled(found)),
        None => {}
    }
    use ion_rs::Value as V;
    match element.value() {
        V::Timestamp(t) => {
            use ion_rs::TimestampPrecision as P;
            match t.precision() {
                P::Year | P::Month => raw_text(body),
                P::Day => Ok(KvValue {
                    kind: KvType::Date,
                    data: KvData::String(crate::storage::ion::timestamp_text(t)),
                }),
                _ => match utc_instant(t) {
                    Some(at) => Ok(KvValue {
                        kind: KvType::Datetime,
                        data: KvData::String(at),
                    }),
                    // A timestamp chrono cannot hold: the text as written.
                    None => raw_text(body),
                },
            }
        }
        V::String(_) | V::Symbol(_) => {
            let data = from_body_element(&element, cap)?;
            let kind = match &data {
                KvData::String(s) if kestra_duration(s).is_some() => KvType::Duration,
                _ => KvType::String,
            };
            Ok(KvValue { kind, data })
        }
        _ => {
            let data = from_body_element(&element, cap)?;
            Ok(KvValue {
                kind: KvType::of_data(&data),
                data,
            })
        }
    }
}

/// [`KvData::from_element`] for an API body: its budget refusal is 413; its
/// `Invalid` cannot occur once [`unmodelled`] found nothing, so it is
/// egret's surprise, answered as an unreadable body.
fn from_body_element(element: &ion_rs::Element, cap: u64) -> Result<KvData, ApiBodyError> {
    KvData::from_element(element, cap).map_err(|e| match e {
        Unstorable::TooLarge(m) => ApiBodyError::TooLarge(m),
        Unstorable::Invalid(m) => ApiBodyError::Crashed(m),
    })
}

/// The whole body as a STRING, or [`ApiBodyError::NotUtf8`] (never a lossy
/// decode: re-review 3, M3).
fn raw_text(body: &[u8]) -> Result<KvValue, ApiBodyError> {
    let text = std::str::from_utf8(body).map_err(|_| ApiBodyError::NotUtf8)?;
    Ok(KvValue {
        kind: KvType::String,
        data: KvData::String(text.to_string()),
    })
}

/// A timestamp of minute precision or finer as an RFC 3339 instant in UTC
/// (nanoseconds kept; [`encode`] writes milliseconds).
fn utc_instant(t: &ion_rs::Timestamp) -> Option<String> {
    let u = t.to_utc();
    let at = chrono::NaiveDate::from_ymd_opt(i32::try_from(u.year()).ok()?, u.month(), u.day())?
        .and_hms_nano_opt(u.hour(), u.minute(), u.second(), t.nanoseconds())?
        .and_utc();
    Some(at.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true))
}

/// §8.1 (W9c Task 5): a typed value and an optional `kvType` -> the
/// [`KvValue`] `Set` stores, as the artifact coerces (`Set.java:116-132`;
/// w9c `set_coercion`):
///
/// - **`kvType` applies to a String only** (review C1, w9c
///   `set_coercion.t_*`): any other value is returned as itself, typed
///   [`KvType::of`], whatever `kvType` says -- except a Number with `STRING`,
///   which becomes its Java `toString()` (`42`, `1.5`, `1.0E20`). So DURATION
///   is the stored type only for a String.
/// - NUMBER: the String read as one JSON number, as Jackson's
///   `readValue(s, Number.class)` does (surrounding whitespace allowed;
///   `1,5`, `0x10`, `+5` refused), then [`KvData::from_json`]: an int or a
///   float, **never a decimal** (the user's ruling, 2026-10-07).
/// - BOOLEAN: strict, `true`/`false` in any case, else the artifact's
///   `Cannot parse '…' as a BOOLEAN value: expected 'true' or 'false'.`
/// - DATETIME: an RFC 3339 instant (an offset is converted to UTC when it is
///   written); else `Cannot convert value '…' to Instant`.
/// - DATE: a `YYYY-MM-DD` date, falling back to an instant's UTC date; else
///   `Text '…' could not be parsed …`.
/// - DURATION: validated as `java.time.Duration.parse` reads it, kept as
///   written; else `Cannot convert value '…' to Duration`.
/// - JSON: the String parsed, typed by what it holds ([`KvType::of`]: `"s"`
///   is a STRING, `7` a NUMBER, `null` a STRING).
/// - STRING: the String.
///
/// Err is typed: `Invalid` for a coercion refusal (its message names the
/// value and the type), `TooLarge` for a number over [`MAX_NUMBER_DIGITS`]
/// digits; a value nesting deeper than `storage::ion::MAX_DEPTH` is
/// `Invalid` ([`KvData::from_json`]).
pub fn coerce(value: Value, kv_type: Option<KvType>) -> Result<KvValue, Unstorable> {
    let (kind, s) = match (&value, kv_type) {
        (Value::String(s), Some(kind)) => (kind, s.as_str()),
        (Value::Number(n), Some(KvType::String)) => {
            let text = n.to_string();
            check_json_digits(&text)?;
            let text = match KvNumber::int(&text) {
                Some(KvNumber::Int(digits)) => digits,
                _ => {
                    let mut out = String::new();
                    crate::runners::env::write_java_double(
                        &mut out,
                        text.parse().unwrap_or(f64::NAN),
                    );
                    out
                }
            };
            return KvValue::from_json(KvType::String, &Value::String(text));
        }
        _ => return KvValue::from_json(KvType::of(&value), &value),
    };
    let invalid = |why: String| Unstorable::Invalid(why);
    match kind {
        KvType::String => KvValue::from_json(KvType::String, &value),
        KvType::Number => {
            let parsed: Value = serde_json::from_str(s)
                .map_err(|e| invalid(format!("Cannot parse '{s}' as a NUMBER value: {e}")))?;
            if !parsed.is_number() {
                return Err(invalid(format!(
                    "Cannot parse '{s}' as a NUMBER value: it is not a number"
                )));
            }
            KvValue::from_json(KvType::Number, &parsed)
        }
        KvType::Boolean => {
            let b = if s.eq_ignore_ascii_case("true") {
                true
            } else if s.eq_ignore_ascii_case("false") {
                false
            } else {
                return Err(invalid(format!(
                    "Cannot parse '{s}' as a BOOLEAN value: expected 'true' or 'false'."
                )));
            };
            Ok(KvValue {
                kind: KvType::Boolean,
                data: KvData::Bool(b),
            })
        }
        KvType::Datetime => match instant(s) {
            Some(_) => KvValue::from_json(KvType::Datetime, &value),
            None => Err(invalid(format!("Cannot convert value '{s}' to Instant"))),
        },
        KvType::Date => {
            let day = date(s).or_else(|| instant(s).map(|t| t.date_naive()));
            match day {
                Some(d) => Ok(KvValue {
                    kind: KvType::Date,
                    data: KvData::String(d.format("%Y-%m-%d").to_string()),
                }),
                None => Err(invalid(format!(
                    "Text '{s}' could not be parsed as a DATE: expected a YYYY-MM-DD date or an \
                     ISO-8601 instant"
                ))),
            }
        }
        KvType::Duration => {
            if java_duration_parses(s) {
                KvValue::from_json(KvType::Duration, &value)
            } else {
                Err(invalid(format!("Cannot convert value '{s}' to Duration")))
            }
        }
        KvType::Json => {
            let parsed: Value = serde_json::from_str(s)
                .map_err(|e| invalid(format!("Cannot parse '{s}' as a JSON value: {e}")))?;
            KvValue::from_json(KvType::of(&parsed), &parsed)
        }
    }
}

/// Whether `java.time.Duration.parse` accepts `s`: `[-+]?P` then days, and
/// after a `T` hours, minutes and seconds (with up to nine fraction digits),
/// each optionally signed, case-insensitive; at least one part, a `T` never
/// alone, and the total in seconds within a Java `long`.
fn java_duration_parses(s: &str) -> bool {
    static PATTERN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"(?i)^[-+]?P(?:([-+]?[0-9]+)D)?(T(?:([-+]?[0-9]+)H)?(?:([-+]?[0-9]+)M)?(?:([-+]?[0-9]+)(?:[.,][0-9]{0,9})?S)?)?$",
        )
        .expect("a fixed pattern")
    });
    let Some(c) = PATTERN.captures(s) else {
        return false;
    };
    let part = |i: usize| c.get(i).map(|m| m.as_str());
    if part(1).is_none() && part(3).is_none() && part(4).is_none() && part(5).is_none() {
        return false;
    }
    if part(2).is_some_and(|t| t.eq_ignore_ascii_case("T")) {
        return false;
    }
    let mut total: i64 = 0;
    for (i, per) in [(1, 86_400i64), (3, 3_600), (4, 60), (5, 1)] {
        let Some(digits) = part(i) else { continue };
        let Some(n) = digits.parse::<i64>().ok().and_then(|n| n.checked_mul(per)) else {
            return false;
        };
        let Some(t) = total.checked_add(n) else {
            return false;
        };
        total = t;
    }
    true
}

/// A flow's JSON number text (`serde_json`, `arbitrary_precision`): its
/// mantissa's digits and its exponent's, each at most [`MAX_NUMBER_DIGITS`].
fn check_json_digits(text: &str) -> Result<(), Unstorable> {
    let (mantissa, exponent) = text
        .split_once(['e', 'E'])
        .map_or((text, ""), |(m, e)| (m, e));
    let digits = |s: &str| s.bytes().filter(u8::is_ascii_digit).count() as u64;
    within_digit_limit(digits(mantissa).max(digits(exponent)))
}

/// A number of `digits` digits, refused (`TooLarge`) past [`MAX_NUMBER_DIGITS`].
fn within_digit_limit(digits: u64) -> Result<(), Unstorable> {
    if digits > MAX_NUMBER_DIGITS {
        return Err(Unstorable::TooLarge(format!(
            "a number of {digits} digits is more than a KV value holds: a KV value's numbers \
             have at most {MAX_NUMBER_DIGITS} digits, and a longer one is refused, never \
             truncated"
        )));
    }
    Ok(())
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
/// Every number is at most [`MAX_NUMBER_DIGITS`] digits as egret writes it
/// (the user's ruling, 2026-10-07): the read back scans egret's own text, so
/// an int or a decimal (in plain notation) longer than that is refused here.
///
/// Err is typed (review P2): [`Unstorable::TooLarge`] for the cap in bytes,
/// the dense-value limit, the decoded-text budget and the digit limit
/// (`NsError::TooLarge`, 413); [`Unstorable::Invalid`] for data not of its
/// type and anything else the read back refuses (`NsError::Author`, 400).
///
/// It runs the Ion reader: call it off the async runtime
/// (`spawn_blocking`), as `store::read_value` runs [`decode`].
pub fn check_readable(v: &KvValue, cap: u64) -> Result<Vec<u8>, Unstorable> {
    let bytes = encode(v).map_err(Unstorable::Invalid)?;
    if bytes.len() as u64 > cap {
        return Err(Unstorable::TooLarge(format!(
            "the {} value is {} bytes as stored, more than workflow.storage.max_read_bytes \
             ({cap} bytes) allows a KV value",
            v.kind,
            bytes.len()
        )));
    }
    read_back(v.kind, &bytes, cap).map_err(|why| {
        why.map(|why| {
            format!(
                "the {} value is not stored: it could not be read back: {why}",
                v.kind
            )
        })
    })?;
    Ok(bytes)
}

/// The bytes back to the [`KvData`] of `kind`: exactly one Ion value through
/// [`for_each_kv_value`] (bounded by `cap` as the preview's reader is, and by
/// [`MAX_NUMBER_DIGITS`]),
/// with nothing [`unmodelled`] in it at any depth, then normalised for
/// DATETIME/DATE as [`encode`] wrote them and for DURATION as w9c
/// `types._rule_read` prints it. A null inside a map is dropped, as every
/// Kestra read drops it (w9c `types.json_nested`). Err names what is wrong,
/// never a storage key.
///
/// CPU-bound: it runs the Ion reader on its own thread and joins it. Call it
/// off the async runtime, as `store::read_value` does.
pub fn decode(kind: KvType, bytes: &[u8], cap: u64) -> Result<KvData, String> {
    read_back(kind, bytes, cap).map_err(|why| why.to_string())
}

/// [`decode`], its refusal typed for [`check_readable`].
fn read_back(kind: KvType, bytes: &[u8], cap: u64) -> Result<KvData, Unstorable> {
    use Unstorable::{Invalid, TooLarge};
    let mut values: Vec<Result<KvData, Unstorable>> = Vec::with_capacity(1);
    let stop = for_each_kv_value(bytes, cap, |element| {
        if !values.is_empty() {
            return ControlFlow::Break(());
        }
        values.push(KvData::from_element(element, cap));
        ControlFlow::Continue(())
    });
    let unread = |e: String| format!("the stored {kind} value cannot be read: {e}");
    match stop {
        Stop::End => {}
        Stop::Full => {
            return Err(Invalid(format!(
                "the stored {kind} value holds more than one Ion value"
            )))
        }
        Stop::TooLarge(e) => return Err(TooLarge(unread(e))),
        Stop::Failed(e) | Stop::TooDeep(e) | Stop::Crashed(e) => return Err(Invalid(unread(e))),
    }
    let data = match values.pop() {
        None => {
            return Err(Invalid(format!(
                "the stored {kind} value holds no Ion value"
            )))
        }
        Some(Err(why)) => {
            return Err(why.map(|why| {
                format!(
                    "the stored {kind} value {why}; egret's KV store writes none, so the object \
                     was not written by it"
                )
            }))
        }
        Some(Ok(data)) => data,
    };
    let invalid = |m: String| Invalid(m);
    // (`KvData` has a `Drop`, so nothing is moved out of it: matched by
    // reference, the whole value returned or a new one built.)
    match (kind, &data) {
        // A rendered `null` is stored as Ion `null`, typed STRING (`KvType::of`).
        (KvType::String, KvData::String(_) | KvData::Null)
        | (KvType::Number, KvData::Number(_))
        | (KvType::Boolean, KvData::Bool(_))
        | (KvType::Json, KvData::Struct(_) | KvData::List(_)) => Ok(data),
        (KvType::Datetime, KvData::String(s)) => instant(s)
            .map(|t| KvData::String(java_instant(t)))
            .ok_or_else(|| invalid(format!("the stored DATETIME value '{s}' is not an instant"))),
        (KvType::Date, KvData::String(s)) => date(s)
            .map(|d| KvData::String(d.format("%Y-%m-%d").to_string()))
            .ok_or_else(|| invalid(format!("the stored DATE value '{s}' is not a date"))),
        (KvType::Duration, KvData::String(s)) => Ok(KvData::String(
            kestra_duration(s).unwrap_or_else(|| s.clone()),
        )),
        (_, d) => Err(invalid(format!(
            "the stored {kind} value is {}, not a {kind}",
            d.shape()
        ))),
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

/// The longest ttl, in days: 1,000 years (Task 3 review I3, spec §4.5,
/// §15). `now() + ttl` then stays far inside Postgres's timestamp range
/// (year 294276).
pub const MAX_TTL_DAYS: i64 = 365_250;

/// [`MAX_TTL_DAYS`] as a duration.
pub fn max_ttl() -> chrono::TimeDelta {
    chrono::TimeDelta::days(MAX_TTL_DAYS)
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
    if d > max_ttl() {
        // Task 3 review I3: past Postgres's timestamp range `now() + ttl`
        // fails as a database error, an outage the run would be re-claimed
        // for forever. A ttl is the author's, so it is bounded here.
        return Err(match mode {
            Ttl::Task => format!(
                "ttl: '{raw}' is longer than {MAX_TTL_DAYS} days (1,000 years), the longest ttl"
            ),
            Ttl::Header => format!(
                "ttl '{raw}' is longer than {MAX_TTL_DAYS} days (1,000 years), the longest ttl"
            ),
        });
    }
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

    /// `f` on a thread with a 2 MiB stack, tokio's blocking-pool size
    /// (Task 3 re-review 2, H1).
    fn on_a_2mib_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(f)
            .unwrap()
            .join()
            .unwrap()
    }

    /// H1: a JSON value nested 10,000 deep is refused by `from_json` (and
    /// `KvValue::from_json`) before anything is built, as `Invalid`, with no
    /// abort; exactly `MAX_DEPTH` is taken. (The `serde_json::Value` itself
    /// is leaked: its own drop recurses, which is not egret's code.)
    #[test]
    fn a_json_value_deeper_than_max_depth_is_refused_by_from_json() {
        use crate::storage::ion::MAX_DEPTH;
        on_a_2mib_stack(|| {
            let nest = |n: usize| {
                let mut v = json!(true);
                for i in 0..n {
                    // (Not `json!`: it serializes an expression, recursively.)
                    v = if i % 2 == 0 {
                        Value::Array(vec![v])
                    } else {
                        Value::Object([("k".to_string(), v)].into_iter().collect())
                    };
                }
                v
            };
            let deep = nest(10_000);
            for got in [
                KvData::from_json(&deep).map(|_| ()),
                KvValue::from_json(KvType::Json, &deep).map(|_| ()),
            ] {
                match got {
                    Err(Unstorable::Invalid(m)) => assert!(
                        m.contains(&format!("deeper than {MAX_DEPTH} levels"))
                            && m.contains("does not store"),
                        "{m}"
                    ),
                    other => panic!("{other:?}"),
                }
            }
            std::mem::forget(deep);
            let at = nest(MAX_DEPTH);
            let d = KvData::from_json(&at).unwrap();
            assert!(!d.nests_deeper_than(MAX_DEPTH) && d.nests_deeper_than(MAX_DEPTH - 1));
            assert!(KvData::from_json(&nest(MAX_DEPTH + 1)).is_err());
        });
    }

    /// H1: a `KvData` nested 100,000 deep, built by hand, drops on a 2 MiB
    /// stack: the `Drop` dismantles it with a heap stack. Lists and structs
    /// alternate.
    #[test]
    fn dropping_a_100000_deep_kv_data_does_not_overflow() {
        on_a_2mib_stack(|| {
            let mut d = KvData::Bool(true);
            for i in 0..100_000 {
                d = if i % 2 == 0 {
                    KvData::List(vec![d, KvData::Null])
                } else {
                    KvData::Struct(vec![
                        ("k".into(), d),
                        ("s".into(), KvData::String("x".into())),
                    ])
                };
            }
            assert!(d.nests_deeper_than(99_999) && !d.nests_deeper_than(100_000));
            drop(d);
        });
    }

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
                    KvData::from_json(&serde_json::from_str(input).unwrap()).unwrap()
                }
                // A DURATION is stored as written.
                KvType::Duration => KvData::from_json(&cell["set"]["value"]).unwrap(),
                _ => KvData::from_json(&get).unwrap(),
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

    /// The one Ion value of `body`, read as the KV store reads Ion and as
    /// Task 6 must read an API body: through [`for_each_kv_value`] (depth
    /// guard, value budget, symbol charge, digit limit), never
    /// `ion_rs::Element::read_one` (review P1). Err is how reading stopped
    /// when it did not end after exactly one value.
    fn one_element(body: &[u8], cap: u64) -> Result<ion_rs::Element, Stop> {
        let mut found = Vec::new();
        let stop = for_each_kv_value(body, cap, |element| {
            if !found.is_empty() {
                return ControlFlow::Break(());
            }
            found.push(element.clone());
            ControlFlow::Continue(())
        });
        match (stop, found.pop()) {
            (Stop::End, Some(element)) => Ok(element),
            (stop, _) => Err(stop),
        }
    }

    /// An API body's data as Task 6 converts it: [`one_element`] (the
    /// hardened reader), then [`KvData::from_element`].
    fn api_body(body: &str) -> KvData {
        let element = one_element(body.as_bytes(), 1 << 20).unwrap();
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
            let v = KvValue::from_json(KvType::Number, &json).unwrap();
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
        let float = KvData::from_json(&serde_json::from_str(BIG).unwrap()).unwrap();
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
            KvData::from_json(&serde_json::from_str(int).unwrap()).unwrap(),
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
            // Refused by the digit limit before the budget is charged
            // (re-review 3, M1): no text is built either way.
            assert!(err.contains("at most 1000 digits"), "{ion}: {err}");
        }
        assert_eq!(
            decode(KvType::Number, b"1d9", 4096).unwrap(),
            dec("1000000000")
        );
    }

    /// Review NI1: the expansion budget is the WHOLE value's, not each
    /// decimal's. Under a 5 000-byte cap six decimals of 901 plain digits
    /// each (each under the digit limit and under the cap alone; 5 406 in
    /// all, from 36 bytes of Ion) are refused; five (4 505) read.
    #[test]
    fn the_decoded_text_of_a_whole_value_is_bounded_by_the_cap() {
        const CAP: u64 = 5000;
        let six = b"[1d900,1d900,1d900,1d900,1d900,1d900]";
        let err = decode(KvType::Json, six, CAP).unwrap_err();
        assert_eq!(
            err,
            "the stored JSON value decodes to more than workflow.storage.max_read_bytes \
             (5000 bytes) of digits and text; egret's KV store writes none, so the object \
             was not written by it"
        );
        let element = one_element(six, CAP).unwrap();
        assert!(
            matches!(
                KvData::from_element(&element, CAP),
                Err(Unstorable::TooLarge(_))
            ),
            "the API path too"
        );
        let KvData::List(items) =
            &decode(KvType::Json, b"[1d900,1d900,1d900,1d900,1d900]", CAP).unwrap()
        else {
            panic!()
        };
        assert_eq!(items.len(), 5);
        assert!(matches!(&items[4], KvData::Number(KvNumber::Decimal(t)) if t.len() == 901));
        // Strings and field names share the same budget: 3 x 2 000 bytes of
        // text under a 5 000-byte budget is refused, 2 x 2 000 reads.
        let s = "x".repeat(2000);
        let three = format!("[\"{s}\",\"{s}\",{{'{s}':1}}]");
        let two = format!("[\"{s}\",{{'{s}':1}}]");
        assert!(
            KvData::from_element(&one_element(three.as_bytes(), 1 << 20).unwrap(), 5000).is_err()
        );
        assert!(KvData::from_element(&one_element(two.as_bytes(), 1 << 20).unwrap(), 5000).is_ok());
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
            let Ok(element) = one_element(body.trim().as_bytes(), 1 << 20) else {
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
        let patch = KvData::from_json(&json!({"b": 2, "e": 0.5})).unwrap();
        let KvData::Struct(patch) = &patch else {
            unreachable!()
        };
        fields.extend(patch.iter().cloned());
        assert_eq!(
            written(KvType::Json, data),
            "{a:1.5,c:{d:123456789012345678901234567890.5},b:2,e:0.5e0}"
        );
    }

    #[test]
    fn floats_stay_floats_and_ints_stay_ints() {
        let v = KvValue::from_json(KvType::Json, &json!({"f": 1.5, "i": 2, "g": 100.0})).unwrap();
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
        let walk = |body: &str| unmodelled(&one_element(body.as_bytes(), 1 << 20).unwrap());
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
        let zeros =
            |n: usize| KvValue::from_json(KvType::Json, &Value::Array(vec![json!(0); n])).unwrap();
        let big = zeros(200_000);
        let bytes = encode(&big).unwrap();
        assert_eq!(bytes.len(), 400_001, "400 KB, far under the cap");
        let Unstorable::TooLarge(err) = check_readable(&big, CAP).unwrap_err() else {
            panic!("the dense-value limit is TooLarge")
        };
        assert!(
            err.starts_with("the JSON value is not stored: it could not be read back: ")
                && err.contains("more than 163840 values")
                && err.contains("workflow.storage.max_read_bytes"),
            "{err}"
        );
        // The control: under the budget, written and read.
        let fits = zeros(150_000);
        let bytes = check_readable(&fits, CAP).unwrap();
        let KvData::List(items) = &decode(KvType::Json, &bytes, CAP).unwrap() else {
            panic!()
        };
        assert_eq!(items.len(), 150_000);
        // Over the cap in bytes: refused naming it, before any decode.
        let long = KvValue::from_json(KvType::String, &json!("x".repeat(5000))).unwrap();
        let err = check_readable(&long, 4096).unwrap_err();
        assert_eq!(
            err,
            Unstorable::TooLarge(
                "the STRING value is 5002 bytes as stored, more than \
                 workflow.storage.max_read_bytes (4096 bytes) allows a KV value"
                    .into()
            )
        );
        // A refusal of the type comes first.
        let bad = KvValue {
            kind: KvType::Duration,
            data: KvData::Bool(true),
        };
        assert!(matches!(
            check_readable(&bad, CAP).unwrap_err(),
            Unstorable::Invalid(m) if m.contains("DURATION")
        ));
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
                KvType::of_data(&KvData::from_json(&v).unwrap()),
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

    fn int(digits: &str) -> KvData {
        KvData::Number(KvNumber::Int(digits.into()))
    }

    /// The user's ruling (2026-10-07), re-review 2 N1: a 70,001-digit
    /// decimal, 70 KB and far under the cap, is REFUSED, never truncated --
    /// unlimited, `ion-rs` 1.0.1 hands back 4,466 of its digits and no
    /// error, which the control shows. The refusal is the reader's, before
    /// `ion-rs` parses the number, so it is fast.
    #[test]
    fn a_decimal_of_70001_digits_is_refused_not_truncated() {
        let body = format!("1.{}", "3".repeat(70_000));
        // The control: W9a's reader, no digit limit, loses digits silently.
        let mut seen = None;
        let stop = crate::storage::ion::for_each_value(body.as_bytes(), 10 << 20, |e| {
            seen = e
                .as_decimal()
                .map(|d| d.coefficient().magnitude().to_string().len());
            ControlFlow::Continue(())
        });
        assert!(matches!(stop, Stop::End), "{stop:?}");
        assert_eq!(
            seen,
            Some(4_465),
            "ion-rs keeps 4,465 of the coefficient's 70,001 digits"
        );
        // The KV path: refused, naming the limit, and fast.
        let started = std::time::Instant::now();
        let err = read_back(KvType::Number, body.as_bytes(), 10 << 20).unwrap_err();
        let Unstorable::TooLarge(why) = &err else {
            panic!("{err:?}")
        };
        assert!(why.contains("more than 1000 digits"), "{why}");
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        assert!(matches!(
            one_element(body.as_bytes(), 10 << 20),
            Err(Stop::TooLarge(_))
        ));
        // And on write: a KvData holding it (as the API would hand Task 3).
        let v = KvValue {
            kind: KvType::Number,
            data: dec(&body),
        };
        assert!(matches!(
            check_readable(&v, 10 << 20),
            Err(Unstorable::TooLarge(m)) if m.contains("more than 1000 digits")
        ));
    }

    /// A 1,001-digit int is refused at write on every path: a flow's value
    /// (`from_json`, before it is anything), an int built directly
    /// (`check_readable` reads egret's own text back), and a stored object
    /// or an API body (`decode`, `one_element`). So is a flow's float of
    /// 1,001 mantissa digits, which `from_json` would otherwise round to a
    /// Double silently, and a decimal of 1,001 digits.
    #[test]
    fn a_number_of_1001_digits_is_refused_at_write() {
        let long = "9".repeat(1001);
        let json: Value = serde_json::from_str(&long).unwrap();
        let err = KvValue::from_json(KvType::Number, &json).unwrap_err();
        assert_eq!(
            err,
            Unstorable::TooLarge(
                "a number of 1001 digits is more than a KV value holds: a KV value's numbers \
                 have at most 1000 digits, and a longer one is refused, never truncated"
                    .into()
            )
        );
        let nested: Value = serde_json::from_str(&format!("{{\"a\":[1,{long}]}}")).unwrap();
        assert!(matches!(
            KvData::from_json(&nested),
            Err(Unstorable::TooLarge(_))
        ));
        for float in [format!("1.{}", "5".repeat(1000)), format!("1e{long}")] {
            let json: Value = serde_json::from_str(&float).unwrap();
            assert!(
                matches!(KvData::from_json(&json), Err(Unstorable::TooLarge(_))),
                "{}",
                &float[..10]
            );
        }
        for data in [
            int(&long),
            int(&format!("-{long}")),
            dec(&format!("1.{}", "5".repeat(1000))),
            dec(&format!("{}.5", "5".repeat(1000))),
        ] {
            let v = KvValue {
                kind: KvType::Number,
                data,
            };
            assert!(
                matches!(check_readable(&v, 1 << 20), Err(Unstorable::TooLarge(m)) if m.contains("1000 digits")),
                "{v:?}"
            );
        }
        assert!(decode(KvType::Number, long.as_bytes(), 1 << 20)
            .unwrap_err()
            .contains("more than 1000 digits"));
        // A decimal whose plain form (as egret writes it) is over the limit
        // is refused at write too, though its Ion coefficient is one digit.
        let v = KvValue {
            kind: KvType::Number,
            data: dec(&format!("1{}", "0".repeat(1000))),
        };
        assert!(matches!(
            check_readable(&v, 1 << 20),
            Err(Unstorable::TooLarge(_))
        ));
    }

    /// The controls at the limit: a 1,000-digit int, a 1,000-digit decimal
    /// (from a flow, from an API body) are written and read back exactly.
    #[test]
    fn a_1000_digit_int_and_decimal_read_back_exactly() {
        let long = "8".repeat(1000);
        let json: Value = serde_json::from_str(&long).unwrap();
        let v = KvValue::from_json(KvType::Number, &json).unwrap();
        assert_eq!(v.data, int(&long));
        let bytes = check_readable(&v, 1 << 20).unwrap();
        let back = decode(KvType::Number, &bytes, 1 << 20).unwrap();
        assert_eq!(back, int(&long));
        assert_eq!(back.to_json().to_string(), long);
        let negative = format!("-{long}");
        assert_eq!(api_body(&negative), int(&negative));
        for plain in [
            format!("{}.{}", "1".repeat(500), "2".repeat(500)),
            format!("-1.{}", "3".repeat(999)),
            format!("0.{}", "4".repeat(999)),
        ] {
            let data = api_body(&plain);
            assert_eq!(data, dec(&plain));
            let bytes = check_readable(
                &KvValue {
                    kind: KvType::Number,
                    data,
                },
                1 << 20,
            )
            .unwrap();
            assert_eq!(String::from_utf8(bytes.clone()).unwrap(), plain);
            let back = decode(KvType::Number, &bytes, 1 << 20).unwrap();
            assert_eq!(back, dec(&plain));
            assert_eq!(back.to_json().to_string(), plain, "every digit");
        }
    }

    /// A binary Ion body or object whose int is longer than a 1,000-digit
    /// number takes is refused before `ion-rs` parses it.
    #[test]
    fn a_binary_ion_long_int_is_refused() {
        let mut long = crate::storage::ion::BINARY_VERSION_MARKER.to_vec();
        // A positive int (0x2E), length 2 000 bytes (VarUInt 0x0F 0xD0).
        long.extend([0x2E, 0x0F, 0xD0]);
        long.extend(std::iter::repeat_n(0xFF, 2000));
        let err = read_back(KvType::Number, &long, 1 << 20).unwrap_err();
        assert!(
            matches!(&err, Unstorable::TooLarge(m) if m.contains("2000 bytes long") && m.contains("at most 1000 digits")),
            "{err:?}"
        );
        assert!(matches!(
            one_element(&long, 1 << 20),
            Err(Stop::TooLarge(_))
        ));
        // The control: a short binary int reads.
        let mut short = crate::storage::ion::BINARY_VERSION_MARKER.to_vec();
        short.extend([0x21, 0x07]);
        assert_eq!(decode(KvType::Number, &short, 4096).unwrap(), int("7"));
    }

    /// Re-review 2, N2: with numbers bounded, a value at the 10 MiB cap made
    /// of 1,000-digit numbers (ints and decimals) decodes fast -- the time is
    /// linear in the value, not quadratic in a number: 0.97 s in a dev build
    /// alone, 2.1 s under the lib suite's parallel load, with `num-bigint`
    /// optimised as `ion-rs` is (the workspace `Cargo.toml`). Unbounded, ONE
    /// 500,000-digit int took 2.4 s to parse in a debug build.
    #[test]
    fn a_cap_size_value_of_1000_digit_numbers_decodes_fast() {
        const CAP: u64 = 10 << 20;
        let i = "9".repeat(1000);
        let d = format!("{}.{}", "1".repeat(500), "2".repeat(500));
        // Each number and its comma are 1 001 or 1 002 bytes.
        let n = (CAP as usize - 2) / 1002;
        let items: Vec<KvData> = (0..n)
            .map(|k| if k % 2 == 0 { int(&i) } else { dec(&d) })
            .collect();
        let v = KvValue {
            kind: KvType::Json,
            data: KvData::List(items),
        };
        let bytes = check_readable(&v, CAP).unwrap();
        let started = std::time::Instant::now();
        let back = decode(KvType::Json, &bytes, CAP).unwrap();
        let took = started.elapsed();
        assert!(
            bytes.len() as u64 > CAP - 10_000,
            "{} bytes: at the cap",
            bytes.len()
        );
        assert_eq!(back, v.data);
        assert!(
            // Generous, so the bound holds under the gate's parallel load
            // (2.1 s measured there); unbounded, one 10 MiB int is minutes.
            took < std::time::Duration::from_secs(8),
            "{n} numbers decoded in {took:?}"
        );
        eprintln!("{n} numbers, {} bytes: {took:?}", bytes.len());
    }

    /// Re-review 2's nit: the budget charge of a decimal is exactly its
    /// text, so a value egret wrote at the cap reads (it charged one byte
    /// more, `len + 2`, and refused `1.` + 999 digits at a cap of 1 001).
    #[test]
    fn a_decimal_written_at_the_cap_reads_back() {
        for plain in [
            format!("1.{}", "5".repeat(999)),
            format!("-1.{}", "5".repeat(999)),
            format!("0.{}1", "0".repeat(998)),
            format!("{}0", "5".repeat(999)),
        ] {
            let v = KvValue {
                kind: KvType::Number,
                data: dec(&plain),
            };
            let cap = encode(&v).unwrap().len() as u64;
            let bytes = check_readable(&v, cap).unwrap_or_else(|e| panic!("{plain}: {e}"));
            assert_eq!(decode(KvType::Number, &bytes, cap).unwrap(), dec(&plain));
        }
    }

    /// Review P2: `check_readable`'s refusal is typed, so Tasks 3, 5 and 6
    /// map a variant and never a message: every bound is `TooLarge`, every
    /// other refusal `Invalid`.
    #[test]
    fn check_readable_names_its_class_by_variant() {
        let too_large = [
            // Over the cap in bytes.
            KvValue::from_json(KvType::String, &json!("x".repeat(5000))).unwrap(),
            // The dense-value limit.
            KvValue::from_json(KvType::Json, &Value::Array(vec![json!(0); 200])).unwrap(),
            // The digit limit.
            KvValue {
                kind: KvType::Number,
                data: int(&"1".repeat(1001)),
            },
        ];
        for v in &too_large {
            assert!(
                matches!(check_readable(v, 4096), Err(Unstorable::TooLarge(_))),
                "{:?}",
                check_readable(v, 4096)
            );
        }
        // Built by hand: `from_json` refuses this depth itself (H1).
        let mut deep = int("1");
        for _ in 0..150 {
            deep = KvData::List(vec![deep]);
        }
        let invalid = [
            KvValue {
                kind: KvType::Duration,
                data: KvData::Bool(true),
            },
            KvValue {
                kind: KvType::Json,
                data: deep,
            },
        ];
        for v in &invalid {
            assert!(
                matches!(check_readable(v, 1 << 20), Err(Unstorable::Invalid(_))),
                "{:?}",
                check_readable(v, 1 << 20)
            );
        }
    }

    /// Re-review 3, M1: reads enforce the digit limit too. What the scan
    /// passes as written but expands past 1,000 digits once parsed is
    /// refused by the conversion, `TooLarge`, before its text is built:
    /// `1d1000` (1,001 plain digits), `1d999999` (a million, from 8 bytes),
    /// `0x` + 1,000 hex digits (1,205 decimal digits) and a 432-byte binary
    /// decimal (about 1,038). Each control at the limit reads.
    #[test]
    fn a_read_refuses_a_number_that_expands_past_1000_digits() {
        let mut binary = crate::storage::ion::BINARY_VERSION_MARKER.to_vec();
        // A decimal (0x5E), length 432 (VarUInt 0x03 0xB0): exponent 0
        // (VarInt 0x80), then a 431-byte coefficient.
        binary.extend([0x5E, 0x03, 0xB0, 0x80]);
        binary.extend(std::iter::repeat_n(0x7F, 431));
        let hex = format!("0x{}", "f".repeat(1000));
        for (name, bytes) in [
            ("1d1000", b"1d1000".to_vec()),
            ("1d999999", b"1d999999".to_vec()),
            ("hex", hex.into_bytes()),
            ("binary decimal", binary),
        ] {
            let element = one_element(&bytes, 1 << 20).unwrap_or_else(|s| panic!("{name}: {s:?}"));
            let err = KvData::from_element(&element, 1 << 20).unwrap_err();
            assert!(
                matches!(&err, Unstorable::TooLarge(m) if m.contains("at most 1000 digits")),
                "{name}: {err:?}"
            );
            let err = read_back(KvType::Number, &bytes, 1 << 20).unwrap_err();
            assert!(matches!(err, Unstorable::TooLarge(_)), "{name}: {err:?}");
        }
        // The controls: exactly 1,000 digits, by exponent and by hex.
        assert_eq!(
            decode(KvType::Number, b"1d999", 4096).unwrap(),
            dec(&format!("1{}", "0".repeat(999)))
        );
        assert_eq!(
            decode(KvType::Number, b"-1d-999", 4096).unwrap(),
            dec(&format!("-0.{}1", "0".repeat(998)))
        );
        let KvData::Number(KvNumber::Int(digits)) = &decode(
            KvType::Number,
            format!("0x{}", "f".repeat(830)).as_bytes(),
            4096,
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(digits.len(), 1000, "16^830 - 1 has 1,000 digits");
    }

    /// Task 3 review I3: a ttl past 1,000 years is the author's, in both
    /// modes, naming `ttl`; at the bound it is accepted.
    #[test]
    fn a_ttl_over_1000_years_is_refused_in_both_modes() {
        for mode in [Ttl::Task, Ttl::Header] {
            for raw in ["P200000000D", "P365251D", "PT31557600001S"] {
                let err = parse_ttl(raw, mode).unwrap_err();
                assert!(
                    err.contains("ttl") && err.contains("365250 days"),
                    "{mode:?} {raw}: {err}"
                );
            }
            assert_eq!(parse_ttl("P365250D", mode), Ok(max_ttl()));
        }
    }

    // ── Task 6: the API's PUT body ───────────────────────────────────────

    /// Every w9c `api.put_bodies` cell through [`parse_api_body`]: a 400 cell
    /// is refused naming what it says; a 200 cell is stored as its type, as
    /// exactly its `ion_bytes`, and read back as its GET value.
    #[test]
    fn every_put_body_cell_parses_to_its_stored_type_bytes_and_value() {
        let cells = fixture()["egret_expected"]["api"]["put_bodies"]
            .as_object()
            .unwrap()
            .clone();
        let mut examined = 0;
        for (name, cell) in cells.iter().filter(|(n, _)| !n.starts_with('_')) {
            examined += 1;
            let body = cell["body"].as_str().unwrap();
            let got = parse_api_body(body.as_bytes(), 1 << 20);
            if cell["put_status"] == 400 {
                let err = got.expect_err(name);
                let msg = err.message();
                for frag in cell["message_contains"].as_array().unwrap() {
                    assert!(msg.contains(frag.as_str().unwrap()), "{name}: {msg}");
                }
                continue;
            }
            assert_eq!(cell["put_status"], 200, "{name}");
            let v = got.unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!(v.kind.as_str(), cell["get"]["type"], "{name}");
            let bytes = check_readable(&v, 1 << 20).unwrap();
            assert_eq!(
                std::str::from_utf8(&bytes).unwrap(),
                cell["ion_bytes"].as_str().unwrap(),
                "{name}"
            );
            let back = decode(v.kind, &bytes, 1 << 20).unwrap();
            assert_eq!(back.to_json(), cell["get"]["value"], "{name}");
        }
        assert_eq!(examined, 54, "every put_bodies cell is examined");
    }

    /// Re-review P1: `ApiBodyError::Unmodelled` never holds `Annotation`:
    /// an annotated body is its raw text, at any depth and on any type.
    #[test]
    fn an_annotated_body_is_the_raw_text_and_never_unmodelled_annotation() {
        for body in [
            "x::1",
            "{a:x::1}",
            "[1,a::[2]]",
            "x::2025-01-01T",
            "a::b::\"s\"",
        ] {
            let v = parse_api_body(body.as_bytes(), 4096).unwrap();
            assert_eq!(v.kind, KvType::String, "{body}");
            assert_eq!(v.data, KvData::String(body.into()), "{body}");
        }
        // The refusal comes first (re-review N3).
        for (body, name) in [
            ("x::{{aGVsbG8=}}", Unmodelled::Blob),
            ("[a::1,(b)]", Unmodelled::SExp),
            ("{a:x::{{\"c\"}}}", Unmodelled::Clob),
        ] {
            assert_eq!(
                parse_api_body(body.as_bytes(), 4096),
                Err(ApiBodyError::Unmodelled(name)),
                "{body}"
            );
        }
    }

    /// Fix round 3 (re-review P1): a reader refusal is never the raw text.
    #[test]
    fn a_reader_refusal_is_its_error_and_never_the_raw_text() {
        let deep = format!("{}1{}", "[".repeat(101), "]".repeat(101));
        assert!(matches!(
            parse_api_body(deep.as_bytes(), 1 << 20),
            Err(ApiBodyError::TooDeep(_))
        ));
        let long = format!("1.{}", "1".repeat(70_000));
        match parse_api_body(long.as_bytes(), 1 << 20) {
            Err(ApiBodyError::TooLarge(m)) => assert!(m.contains("1000"), "{m}"),
            other => panic!("{other:?}"),
        }
        // A value over the cap is the reader's TooLarge, not the raw text.
        let big = format!("\"{}\"", "a".repeat(5000));
        assert!(matches!(
            parse_api_body(big.as_bytes(), 4096),
            Err(ApiBodyError::TooLarge(_))
        ));
        // Expanding decimals share one budget (fix round 2, NI1).
        assert!(matches!(
            parse_api_body(b"[1d9000000,1d9000000]", 10 << 20),
            Err(ApiBodyError::TooLarge(_))
        ));
        // The huge symbol id is refused unread (re-review 3, M2): raw text.
        let v = parse_api_body(b"$99999999999999999999", 4096).unwrap();
        assert_eq!(v.data, KvData::String("$99999999999999999999".into()));
        // The controls: 1,000 digits round-trip exactly.
        let int = "9".repeat(1000);
        let v = parse_api_body(int.as_bytes(), 1 << 20).unwrap();
        assert_eq!(v.data, KvData::Number(KvNumber::Int(int.clone())));
        let dec_text = format!("{}.5", "9".repeat(998));
        let v = parse_api_body(dec_text.as_bytes(), 1 << 20).unwrap();
        assert_eq!(v.data, dec(&dec_text));
    }

    /// Re-review 3, M3: a raw-text fallback that is not UTF-8 is refused.
    #[test]
    fn a_raw_text_fallback_that_is_not_utf8_is_not_utf8() {
        let mut binary = crate::storage::ion::BINARY_VERSION_MARKER.to_vec();
        binary.push(0xF0); // a reserved type code
        for body in [binary.as_slice(), &[0xFF, 0xFE, 0x00][..]] {
            assert_eq!(
                parse_api_body(body, 4096),
                Err(ApiBodyError::NotUtf8),
                "{body:?}"
            );
        }
        assert_eq!(
            parse_api_body(b"{a:", 4096).unwrap().data,
            KvData::String("{a:".into())
        );
    }

    /// R-b: no value at all is Empty, whatever the whitespace.
    #[test]
    fn an_empty_or_blank_body_is_empty() {
        for body in ["", " ", "\n\t\r ", "  \u{0b}\u{0c} "] {
            assert_eq!(
                parse_api_body(body.as_bytes(), 4096),
                Err(ApiBodyError::Empty),
                "{body:?}"
            );
        }
    }
}
