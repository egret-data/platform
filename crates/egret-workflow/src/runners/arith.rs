//! Package #5a, Task A3 (spec D2): **arithmetic is Pebble's.**
//!
//! minijinja's `+ - * / %` are Python's: `7 / 2` is `3.5`, an integer past
//! `i64` widens to `i128`, `'5' * 2` repeats the string and `1 + 'a'` is
//! refused. Kestra renders with Pebble 4.1.2, whose `AddExpression` &co. call
//! `io.pebbletemplates.pebble.utils.OperatorUtils` (read from the pinned jar
//! with `javap -c -p`) and wrap any exception it throws in a `PebbleException`
//! with a fixed text. [`super::cmp_rewrite::compile_arithmetic`] (the third
//! compile pass) turns every binary operator into a call to one of the five
//! functions here, which port `OperatorUtils`:
//!
//! - **`add`**: if either operand is a String, `String.valueOf(left) +
//!   String.valueOf(right)` (`1 + 'a'` is `1a`, `'a' + null` is `anull`,
//!   `'a' + [1, 2]` is `a[1, 2]`); else if the left is a List, a copy with the
//!   right appended (`addAll` when the right is a List too); else numeric.
//! - **`subtract`**: a List on the left is a copy with the right removed --
//!   `removeAll` for a List, else `remove(Object)`, the FIRST equal element
//!   (`[1, 2, 1] - 1` is `[2, 1]`), equality being Java's `equals`; else
//!   numeric.
//! - **`multiply`, `divide`, `mod`**: numeric only (`[1] * 2` fails).
//! - **numeric** (`wideningConversionBinaryOperation`): each operand goes
//!   through `toNumber` -- a Number is itself; a String containing `.`, `e`
//!   or `E` is `Double.parseDouble`, any other String `Long.parseLong`;
//!   anything else (null, a boolean, a list, a map) throws. Then the first
//!   branch that matches, in this order: either a `BigDecimal` (Task A4; see
//!   below), either a `Double` (`double` arithmetic: `1.0 / 0` is
//!   `Infinity`), either a `Long` (`long` arithmetic through `longValue()`:
//!   wrapping `+ - *`, truncating `/`, Java's `%` sign rule, `/ 0` and `% 0`
//!   throw), else `int` arithmetic through `intValue()`, widened to `long`.
//!
//! Every failure is Kestra's text verbatim: `Could not perform addition`,
//! `subtraction`, `multiplication`, `division`, and `greater modulus` (sic,
//! that is `ModulusExpression`'s string).
//!
//! **What egret cannot carry: Integer vs Long** (spec D2, a recorded
//! divergence). A template literal is a `Long` in Pebble, but a context value
//! Jackson read from JSON is an `Integer` when it fits 32 bits, and two
//! `Integer`s take the last (`int`) branch, so `{{ a * b }}` with two context
//! values `65536` is `0` in Kestra. egret's integers carry no such
//! distinction; every one that fits 64 bits is a `Long` here
//! (`arith-operand-context-ints-*` stay baselined). The same distinction makes
//! Java's `equals` in `[..] - x` false between an `Integer` and a `Long`;
//! here they are equal.
//!
//! **An integer past `i64`** is a `java.math.BigInteger` in Kestra (Jackson,
//! or a `kv()` value), and it takes the `Long` branch through `longValue()`,
//! its low 64 bits (A1 measured `99999999999999999999 + 1` as
//! `7766279631452241920`), the `Double` branch through `doubleValue()`, and
//! with another `BigInteger` the `int` branch through `intValue()`.
//!
//! **A `BigDecimal` operand** (package #5a Task A4, spec D3 and Q5, which
//! reverses W9c's refusal): a `kv()` decimal, `number(type='BIGDECIMAL')`, an
//! Ion decimal from `fromIon` and the result of an operation on any of them.
//! `wideningConversionBinaryOperation` turns BOTH operands into
//! `BigDecimal.valueOf(x.doubleValue())` -- so a 30-digit value loses its
//! digits past a double's 17th, and `1.50 + 1` is `2.5` -- and
//! `bigDecimalOperation` computes `add`/`subtract` exactly and `multiply`/
//! `divide`/`remainder` at `MathContext.DECIMAL128` (34 digits, `HALF_EVEN`)
//! with Java's preferred scale: `1.5 * 2` is `1.5 * 2.0`, so `3.00`; `1.0 / 3`
//! is 34 threes; `1.5 / 2` is the exact `0.75`. [`decimal`] is the port, on
//! `num-bigint`. `/ 0`, `% 0` and a NaN or infinite operand (`valueOf` throws)
//! fail with the operator's text.

use minijinja::value::{Value as MjValue, ValueKind};
use minijinja::{Environment, Error as MjError, ErrorKind, State};

pub(super) use super::cmp_rewrite::compile_arithmetic;

/// A binary arithmetic operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

impl ArithOp {
    /// The function the operator becomes.
    pub(super) fn function(self) -> &'static str {
        match self {
            ArithOp::Add => "kestra_add",
            ArithOp::Sub => "kestra_sub",
            ArithOp::Mul => "kestra_mul",
            ArithOp::Div => "kestra_div",
            ArithOp::Mod => "kestra_mod",
        }
    }

    /// The text Pebble's `AddExpression` &co. wrap `OperatorUtils`' failure
    /// in, verbatim.
    pub(super) fn kestra_refusal(self) -> &'static str {
        match self {
            ArithOp::Add => "Could not perform addition",
            ArithOp::Sub => "Could not perform subtraction",
            ArithOp::Mul => "Could not perform multiplication",
            ArithOp::Div => "Could not perform division",
            ArithOp::Mod => "Could not perform greater modulus",
        }
    }

    fn refused(self) -> MjError {
        MjError::new(ErrorKind::InvalidOperation, self.kestra_refusal())
    }

    /// One operation, with the VM's undefined checks first (left, then
    /// right, as Pebble evaluates them).
    fn apply(self, state: &State, left: &MjValue, right: &MjValue) -> Result<MjValue, MjError> {
        super::compare::assert_defined(state, left)?;
        super::compare::assert_defined(state, right)?;
        match self {
            ArithOp::Add if is_string(left) || is_string(right) => {
                let mut text = java_string_value_of(left);
                text.push_str(&java_string_value_of(right));
                Ok(MjValue::from(text))
            }
            ArithOp::Add if is_list(left) => {
                let mut items = list_items(left);
                if is_list(right) {
                    let more = list_items(right);
                    let total = items.len().saturating_add(more.len());
                    check_list_length(total)?;
                    charge_list_work(state, total as u64)?;
                    items.extend(more);
                } else {
                    check_list_length(items.len().saturating_add(1))?;
                    charge_list_work(state, items.len() as u64 + 1)?;
                    items.push(right.clone());
                }
                Ok(MjValue::from(items))
            }
            ArithOp::Sub if is_list(left) => {
                let mut items = list_items(left);
                if is_list(right) {
                    let remove = list_items(right);
                    // `removeAll`: every element against every one removed.
                    charge_list_work(state, pair_work(items.len(), remove.len()))?;
                    items.retain(|item| !remove.iter().any(|r| java_equals(item, r)));
                } else {
                    charge_list_work(state, items.len() as u64)?;
                    if let Some(at) = items.iter().position(|item| java_equals(right, item)) {
                        items.remove(at);
                    }
                }
                Ok(MjValue::from(items))
            }
            _ => self.numeric(left, right),
        }
    }

    /// `wideningConversionBinaryOperation`.
    fn numeric(self, left: &MjValue, right: &MjValue) -> Result<MjValue, MjError> {
        let a = to_number(left).ok_or_else(|| self.refused())?;
        let b = to_number(right).ok_or_else(|| self.refused())?;
        if matches!(a, Operand::BigDecimal(_)) || matches!(b, Operand::BigDecimal(_)) {
            return self
                .big_decimal(a.double_value(), b.double_value())
                .ok_or_else(|| self.refused());
        }
        if matches!(a, Operand::Double(_)) || matches!(b, Operand::Double(_)) {
            return Ok(MjValue::from(
                self.double(a.double_value(), b.double_value()),
            ));
        }
        if matches!(a, Operand::Long(_)) || matches!(b, Operand::Long(_)) {
            return self
                .long(a.long_value(), b.long_value())
                .map(MjValue::from)
                .ok_or_else(|| self.refused());
        }
        self.int(a.long_value() as i32, b.long_value() as i32)
            .map(MjValue::from)
            .ok_or_else(|| self.refused())
    }

    /// `bigDecimalOperation(BigDecimal.valueOf(a), BigDecimal.valueOf(b))`,
    /// the result a `java.math.BigDecimal` [`super::filters::big_decimal`].
    /// `None` is the exception: `valueOf` of a NaN or an infinity, a zero
    /// divisor, `Division impossible`, or a result over the output cap.
    fn big_decimal(self, a: f64, b: f64) -> Option<MjValue> {
        let (a, b) = (decimal::Dec::value_of(a)?, decimal::Dec::value_of(b)?);
        let result = match self {
            ArithOp::Add => a.add(&b),
            ArithOp::Sub => a.subtract(&b),
            ArithOp::Mul => a.multiply(&b),
            ArithOp::Div => a.divide(&b, decimal::Rounding::HalfEven)?,
            ArithOp::Mod => a.remainder(&b)?,
        };
        super::filters::big_decimal(&result.scientific())
    }

    /// `doubleOperation`: IEEE 754, Java's `%` (`drem` is `fmod`, which is
    /// Rust's `%` on `f64`).
    fn double(self, a: f64, b: f64) -> f64 {
        match self {
            ArithOp::Add => a + b,
            ArithOp::Sub => a - b,
            ArithOp::Mul => a * b,
            ArithOp::Div => a / b,
            ArithOp::Mod => a % b,
        }
    }

    /// `longOperation`: `ladd`/`lsub`/`lmul` wrap, `ldiv`/`lrem` truncate
    /// toward zero and throw on a zero divisor; `Long.MIN_VALUE / -1` is
    /// `Long.MIN_VALUE` and `% -1` is `0` (`wrapping_*`). `None` is the
    /// `ArithmeticException`.
    fn long(self, a: i64, b: i64) -> Option<i64> {
        match self {
            ArithOp::Add => Some(a.wrapping_add(b)),
            ArithOp::Sub => Some(a.wrapping_sub(b)),
            ArithOp::Mul => Some(a.wrapping_mul(b)),
            ArithOp::Div => (b != 0).then(|| a.wrapping_div(b)),
            ArithOp::Mod => (b != 0).then(|| a.wrapping_rem(b)),
        }
    }

    /// `integerOperation`: the same in 32 bits, widened (`i2l`) to a long.
    fn int(self, a: i32, b: i32) -> Option<i64> {
        let result = match self {
            ArithOp::Add => a.wrapping_add(b),
            ArithOp::Sub => a.wrapping_sub(b),
            ArithOp::Mul => a.wrapping_mul(b),
            ArithOp::Div => (b != 0).then(|| a.wrapping_div(b))?,
            ArithOp::Mod => (b != 0).then(|| a.wrapping_rem(b))?,
        };
        Some(i64::from(result))
    }
}

/// What `toNumber` makes of an operand, by the Java class that decides the
/// branch.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Operand {
    Long(i64),
    Double(f64),
    /// A `java.math.BigInteger`: its `longValue()` (the low 64 bits) and its
    /// `doubleValue()` (rounded to nearest).
    BigInteger {
        low: i64,
        double: f64,
    },
    /// A `java.math.BigDecimal`: its `doubleValue()` (rounded to nearest),
    /// which is all `wideningConversionBinaryOperation` reads of it.
    BigDecimal(f64),
}

impl Operand {
    fn double_value(self) -> f64 {
        match self {
            Operand::Long(v) => v as f64,
            Operand::Double(v) => v,
            Operand::BigInteger { double, .. } => double,
            Operand::BigDecimal(double) => double,
        }
    }

    fn long_value(self) -> i64 {
        match self {
            Operand::Long(v) => v,
            // `(long) d`: truncation toward zero, saturating, NaN is 0 --
            // which is Rust's `as`. Unreachable from `numeric` (a Double
            // takes the Double branch) and kept total.
            Operand::Double(v) => v as i64,
            Operand::BigInteger { low, .. } => low,
            Operand::BigDecimal(double) => double as i64,
        }
    }
}

/// `OperatorUtils.toNumber`; `None` is the exception it throws (or that
/// `parseDouble`/`parseLong` throw).
fn to_number(value: &MjValue) -> Option<Operand> {
    if let Some(class) = super::filters::big_number_class(value) {
        let digits = super::filters::big_number_plain(value)?;
        if class == "java.math.BigDecimal" {
            // `BigDecimal.doubleValue()` is correctly rounded (JDK 9+), as
            // Rust's parse of the plain digits is.
            return digits.parse::<f64>().ok().map(Operand::BigDecimal);
        }
        return big_integer_of_digits(digits);
    }
    match value.kind() {
        ValueKind::Number if value.is_integer() => {
            if let Ok(v) = i64::try_from(value.clone()) {
                return Some(Operand::Long(v));
            }
            if let Ok(v) = i128::try_from(value.clone()) {
                return Some(Operand::BigInteger {
                    low: v as i64,
                    double: v as f64,
                });
            }
            u128::try_from(value.clone())
                .ok()
                .map(|v| Operand::BigInteger {
                    low: v as i64,
                    double: v as f64,
                })
        }
        ValueKind::Number => f64::try_from(value.clone()).ok().map(Operand::Double),
        ValueKind::String => {
            let text = value.as_str()?;
            if text.contains(['.', 'e', 'E']) {
                java_parse_double(text).map(Operand::Double)
            } else {
                // `Long.parseLong`: an optional sign and decimal digits, no
                // whitespace, overflow throws -- Rust's `i64::from_str`.
                // (Java also takes non-ASCII Unicode digits; Rust does not.)
                text.parse::<i64>().ok().map(Operand::Long)
            }
        }
        _ => None,
    }
}

/// A `BigInteger`'s `longValue()` and `doubleValue()` from its digits
/// (`-?[0-9]+`, past every Rust integer primitive).
fn big_integer_of_digits(digits: &str) -> Option<Operand> {
    let (negative, body) = match digits.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, digits),
    };
    if body.is_empty() || !body.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // The value modulo 2^64, which is what `longValue()` keeps.
    let mut low: u64 = 0;
    for b in body.bytes() {
        low = low.wrapping_mul(10).wrapping_add(u64::from(b - b'0'));
    }
    let low = if negative {
        (low as i64).wrapping_neg()
    } else {
        low as i64
    };
    let double = digits.parse::<f64>().ok()?;
    Some(Operand::BigInteger { low, double })
}

/// `Double.parseDouble` for the decimal forms: `String.trim()` (every char
/// up to U+0020), an optional sign, digits with an optional `.` (at least
/// one digit), an optional exponent, and an optional `f`/`F`/`d`/`D` type
/// suffix, which does not change the value. Java's hexadecimal form
/// (`0x1.8p1`) and the words `NaN`/`Infinity` are not ported: the words never
/// reach here (`toNumber` sends only a string with `.`, `e` or `E`), and
/// the hexadecimal form fails here where Java parses it.
fn java_parse_double(text: &str) -> Option<f64> {
    let trimmed = text.trim_matches(|c: char| c <= ' ');
    let body = trimmed
        .strip_suffix(['f', 'F', 'd', 'D'])
        .unwrap_or(trimmed);
    let unsigned = body.strip_prefix(['+', '-']).unwrap_or(body);
    let (mantissa, exponent) = match unsigned.find(['e', 'E']) {
        Some(at) => (&unsigned[..at], Some(&unsigned[at + 1..])),
        None => (unsigned, None),
    };
    let (int_part, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if int_part.len() + fraction.len() == 0 || !digits(int_part) || !digits(fraction) {
        return None;
    }
    if let Some(exponent) = exponent {
        let exponent = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
        if exponent.is_empty() || !digits(exponent) {
            return None;
        }
    }
    // Rust's parse is correctly rounded, as Java's is.
    body.parse::<f64>().ok()
}

fn is_string(value: &MjValue) -> bool {
    value.kind() == ValueKind::String
}

fn is_list(value: &MjValue) -> bool {
    matches!(value.kind(), ValueKind::Seq | ValueKind::Iterable)
}

fn list_items(value: &MjValue) -> Vec<MjValue> {
    value.try_iter().map(Iterator::collect).unwrap_or_default()
}

/// **egret-only bounds on the list operators** (fix wave #5a, I1; spec §6.1
/// "Bounds on the list operators"). Kestra has neither.
///
/// `ArrayList.removeAll` and `containsAll` compare every element of one list
/// with every element of the other, so `(0..999999) - (0..999999)` -- 35
/// bytes of template, or of webhook data a fixpoint render evaluates -- is
/// 10^12 comparisons (measured: 30 000 x 30 000 took 5.9 s, so that one is
/// about 1.8 h). Fuel cannot stop it: a function call is one VM instruction.
/// And list `+` materialises its result, so ten `{% set l = l + l %}` on a
/// million-element list is 10^9 values, an OOM kill of the whole worker.
///
/// Two ceilings, each named in the refusal:
///
/// - [`MAX_LIST_OPERATOR_WORK`]: every list operator (`+`, `-`, `contains`)
///   charges what it will do -- `n x m` element comparisons for `removeAll`
///   and `containsAll`, `n` for a scan or a copy -- to ONE budget for the
///   whole render, before doing it. A budget per render rather than per call,
///   because fuel lets 50 000 calls through and 50 000 x anything-per-call is
///   no bound at all. Ten million is ~70 ms of comparisons (measured
///   10^8 in 0.66 s).
/// - [`MAX_LIST_OPERATOR_LENGTH`]: no list operator builds a list longer than
///   a million elements, Kestra's own `range` ceiling
///   (`BoundedRangeFunction.MAX_RANGE_SIZE`), so a list operator can build
///   nothing `..` could not.
pub(super) const MAX_LIST_OPERATOR_WORK: u64 = 10_000_000;

/// See [`MAX_LIST_OPERATOR_WORK`].
pub(super) const MAX_LIST_OPERATOR_LENGTH: usize = 1_000_000;

/// The per-render tally behind [`MAX_LIST_OPERATOR_WORK`], kept in the
/// render's temps (one `State` per render, shared by every nested call).
#[derive(Debug, Default)]
struct ListOperatorWork(std::sync::atomic::AtomicU64);

impl minijinja::value::Object for ListOperatorWork {}

/// `n x m` comparisons, never less than the `n` the copy itself costs.
pub(super) fn pair_work(n: usize, m: usize) -> u64 {
    (n as u64).saturating_mul(m as u64).max(n as u64)
}

/// Charge `units` of list work to this render, or refuse BEFORE the work.
pub(super) fn charge_list_work(state: &State, units: u64) -> Result<(), MjError> {
    use std::sync::atomic::Ordering;
    let tally = state.get_or_set_temp_object("egret.list_operator_work", ListOperatorWork::default);
    let mut spent = tally.0.load(Ordering::Relaxed);
    loop {
        let after = spent.saturating_add(units);
        if after > MAX_LIST_OPERATOR_WORK {
            return Err(super::filters::pebble_error(format!(
                "egret refuses list operations that compare or copy more than \
                 {MAX_LIST_OPERATOR_WORK} elements in one render; this one would bring the \
                 total to {after}"
            )));
        }
        match tally
            .0
            .compare_exchange(spent, after, Ordering::Relaxed, Ordering::Relaxed)
        {
            Ok(_) => return Ok(()),
            Err(now) => spent = now,
        }
    }
}

/// Refuse a list operator result past [`MAX_LIST_OPERATOR_LENGTH`].
fn check_list_length(len: usize) -> Result<(), MjError> {
    if len > MAX_LIST_OPERATOR_LENGTH {
        return Err(super::filters::pebble_error(format!(
            "egret refuses a list operation whose result would have more than \
             {MAX_LIST_OPERATOR_LENGTH} elements; this one would have {len}"
        )));
    }
    Ok(())
}

/// `String.valueOf(Object)`: `"null"` for a null (a silent undefined is
/// egret's spelling of that null), else the value's Java `toString()`.
pub(super) fn java_string_value_of(value: &MjValue) -> String {
    if value.is_undefined() || value.is_none() {
        return "null".to_string();
    }
    super::env::kestra_to_string(value)
}

/// Java's `Object.equals` for the values a template holds, which is what
/// `List.remove(Object)` and `removeAll` test: a `Long` never equals a
/// `Double` (`1` is not `1.0`), a `Double` equals by its bits (`NaN` equals
/// `NaN`, `0.0` is not `-0.0`), a `BigDecimal` by value AND scale, lists
/// element by element, maps entry by entry.
pub(super) fn java_equals(a: &MjValue, b: &MjValue) -> bool {
    let (ca, cb) = (
        super::filters::big_number_class(a),
        super::filters::big_number_class(b),
    );
    if ca.is_some() || cb.is_some() {
        // `toString()` is one-to-one on (unscaled, scale); the plain text is
        // not (`1E+2` and `100` are both `100`).
        return ca == cb
            && super::filters::big_number_text(a) == super::filters::big_number_text(b);
    }
    match (a.kind(), b.kind()) {
        (ValueKind::None | ValueKind::Undefined, ValueKind::None | ValueKind::Undefined) => true,
        (ValueKind::Number, ValueKind::Number) => match (a.is_integer(), b.is_integer()) {
            (true, true) => a == b,
            (false, false) => match (f64::try_from(a.clone()), f64::try_from(b.clone())) {
                (Ok(x), Ok(y)) => java_double_bits(x) == java_double_bits(y),
                _ => false,
            },
            _ => false,
        },
        (ValueKind::String, ValueKind::String) | (ValueKind::Bool, ValueKind::Bool) => a == b,
        (ValueKind::Seq | ValueKind::Iterable, ValueKind::Seq | ValueKind::Iterable) => {
            let (x, y) = (list_items(a), list_items(b));
            x.len() == y.len() && x.iter().zip(&y).all(|(p, q)| java_equals(p, q))
        }
        (ValueKind::Map, ValueKind::Map) => {
            let keys = |v: &MjValue| -> Vec<MjValue> {
                v.try_iter().map(Iterator::collect).unwrap_or_default()
            };
            let (ka, kb) = (keys(a), keys(b));
            ka.len() == kb.len()
                && ka.iter().all(|key| {
                    let item = |v: &MjValue| v.get_item(key).ok().filter(|i| !i.is_undefined());
                    match (item(a), item(b)) {
                        (Some(x), Some(y)) => java_equals(&x, &y),
                        _ => false,
                    }
                })
        }
        _ => false,
    }
}

/// `Double.doubleToLongBits`: every NaN is the one canonical NaN.
fn java_double_bits(x: f64) -> u64 {
    if x.is_nan() {
        0x7ff8_0000_0000_0000
    } else {
        x.to_bits()
    }
}

/// The part of `java.math.BigDecimal` (JDK 25) that Pebble's
/// `bigDecimalOperation` reaches, ported on `num-bigint`: a value is an
/// unscaled integer and a scale, `unscaled × 10^-scale`, and the scale is
/// part of the value's text (`1.5` and `1.50` print differently).
///
/// - [`Dec::value_of`] is `BigDecimal.valueOf(double)`, which is
///   `new BigDecimal(Double.toString(d))` -- egret's `Double.toString` port
///   ([`crate::runners::env::write_java_double`]), parsed;
/// - `add`/`subtract` are exact, at the larger scale;
/// - `multiply`, `divide` and `remainder` take `MathContext.DECIMAL128`: a
///   result over 34 significant digits is rounded `HALF_EVEN` (`multiply`'s
///   `doRound`); an exact quotient is stripped of trailing zeros down to the
///   preferred scale `this.scale - divisor.scale` and never below it, an
///   inexact one has exactly 34 digits; `remainder` is `this -
///   divideToIntegralValue(divisor, mc) × divisor`.
mod decimal {
    use std::cmp::Ordering;

    use num_bigint::{BigInt, Sign};

    /// `MathContext.DECIMAL128.getPrecision()`.
    const PRECISION: u64 = 34;

    /// The two `RoundingMode`s the operations use.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) enum Rounding {
        /// `DECIMAL128`'s.
        HalfEven,
        /// `divideToIntegralValue`'s inner division.
        Down,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct Dec {
        unscaled: BigInt,
        scale: i64,
    }

    fn pow10(n: u64) -> BigInt {
        BigInt::from(10u8).pow(u32::try_from(n).expect("a bounded exponent"))
    }

    /// `BigInteger`'s decimal precision; `1` for zero, as Java's.
    fn precision(v: &BigInt) -> u64 {
        v.magnitude().to_str_radix(10).len() as u64
    }

    fn is_zero(v: &BigInt) -> bool {
        v.sign() == Sign::NoSign
    }

    /// `num / den` rounded by `mode`, and whether it was exact.
    fn div_round(num: &BigInt, den: &BigInt, mode: Rounding) -> (BigInt, bool) {
        // `BigInt` division truncates toward zero, as Java's does.
        let (mut q, r) = (num / den, num % den);
        if is_zero(&r) {
            return (q, true);
        }
        if mode == Rounding::HalfEven {
            let twice = r.magnitude() * 2u8;
            let up = match twice.cmp(den.magnitude()) {
                Ordering::Greater => true,
                Ordering::Equal => q.magnitude().bit(0),
                Ordering::Less => false,
            };
            if up {
                let negative = (num.sign() == Sign::Minus) != (den.sign() == Sign::Minus);
                q += if negative { -1 } else { 1 };
            }
        }
        (q, false)
    }

    impl Dec {
        /// `new BigDecimal(String)` for the forms `Double.toString` writes:
        /// `1.5`, `-0.0`, `1.2345678901234568E29`, `1.0E-7`.
        fn parse(text: &str) -> Option<Dec> {
            let (mantissa, exponent) = match text.split_once(['E', 'e']) {
                Some((m, e)) => (m, e.parse::<i64>().ok()?),
                None => (text, 0),
            };
            let (int, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
            let unscaled = format!("{int}{fraction}").parse::<BigInt>().ok()?;
            Some(Dec {
                unscaled,
                scale: fraction.len() as i64 - exponent,
            })
        }

        /// `BigDecimal.valueOf(double)`; `None` for a NaN or an infinity,
        /// where Java throws `NumberFormatException`.
        pub(super) fn value_of(d: f64) -> Option<Dec> {
            if !d.is_finite() {
                return None;
            }
            let mut text = String::new();
            crate::runners::env::write_java_double(&mut text, d);
            Dec::parse(&text)
        }

        /// `unscaledE-scale`, which `filters::big_decimal` reads back with
        /// this exact scale.
        pub(super) fn scientific(&self) -> String {
            format!("{}E{}", self.unscaled, -self.scale)
        }

        /// The unscaled value at `scale` (not below `self.scale`).
        fn at_scale(&self, scale: i64) -> BigInt {
            &self.unscaled * pow10((scale - self.scale) as u64)
        }

        fn compare_magnitude(&self, other: &Dec) -> Ordering {
            let scale = self.scale.max(other.scale);
            self.at_scale(scale)
                .magnitude()
                .cmp(other.at_scale(scale).magnitude())
        }

        pub(super) fn add(&self, other: &Dec) -> Dec {
            let scale = self.scale.max(other.scale);
            Dec {
                unscaled: self.at_scale(scale) + other.at_scale(scale),
                scale,
            }
        }

        pub(super) fn subtract(&self, other: &Dec) -> Dec {
            let scale = self.scale.max(other.scale);
            Dec {
                unscaled: self.at_scale(scale) - other.at_scale(scale),
                scale,
            }
        }

        fn exact_multiply(&self, other: &Dec) -> Dec {
            Dec {
                unscaled: &self.unscaled * &other.unscaled,
                scale: self.scale + other.scale,
            }
        }

        /// `multiply(other, DECIMAL128)`: the exact product, then `doRound`.
        pub(super) fn multiply(&self, other: &Dec) -> Dec {
            let mut result = self.exact_multiply(other);
            loop {
                let digits = precision(&result.unscaled);
                if digits <= PRECISION {
                    return result;
                }
                let drop = digits - PRECISION;
                let (q, _) = div_round(&result.unscaled, &pow10(drop), Rounding::HalfEven);
                result = Dec {
                    unscaled: q,
                    scale: result.scale - drop as i64,
                };
            }
        }

        /// `stripZerosToMatchScale`: trailing zeros off while the scale is
        /// above `preferred` (a zero, under ten, keeps its scale).
        fn strip_to(mut self, preferred: i64) -> Dec {
            let ten = BigInt::from(10u8);
            while self.scale > preferred && self.unscaled.magnitude() >= ten.magnitude() {
                if !is_zero(&(&self.unscaled % &ten)) {
                    break;
                }
                self.unscaled /= &ten;
                self.scale -= 1;
            }
            self
        }

        /// `divide(divisor, new MathContext(34, mode))`; `None` for a zero
        /// divisor.
        pub(super) fn divide(&self, divisor: &Dec, mode: Rounding) -> Option<Dec> {
            if is_zero(&divisor.unscaled) {
                return None;
            }
            let preferred = self.scale - divisor.scale;
            if is_zero(&self.unscaled) {
                return Some(Dec {
                    unscaled: BigInt::from(0u8),
                    scale: preferred,
                });
            }
            // `unscaled × 10^shift / divisor.unscaled` has 34 or 35 digits
            // at the first shift; one step down makes it 34.
            let mut shift = PRECISION as i64 - precision(&self.unscaled) as i64
                + precision(&divisor.unscaled) as i64;
            loop {
                let (num, den) = if shift >= 0 {
                    (
                        &self.unscaled * pow10(shift as u64),
                        divisor.unscaled.clone(),
                    )
                } else {
                    (
                        self.unscaled.clone(),
                        &divisor.unscaled * pow10(shift.unsigned_abs()),
                    )
                };
                if precision(&(&num / &den)) > PRECISION {
                    shift -= 1;
                    continue;
                }
                let (q, exact) = div_round(&num, &den, mode);
                let result = Dec {
                    unscaled: q,
                    scale: shift + preferred,
                };
                if exact {
                    return Some(result.strip_to(preferred));
                }
                if precision(&result.unscaled) > PRECISION {
                    // Rounded up to 10^34: one digit off, exactly.
                    return Some(Dec {
                        unscaled: result.unscaled / 10u8,
                        scale: result.scale - 1,
                    });
                }
                return Some(result);
            }
        }

        /// `divideToIntegralValue(divisor, DECIMAL128)`; `None` for a zero
        /// divisor or `Division impossible` (an integer part over 34 digits).
        fn divide_to_integral_value(&self, divisor: &Dec) -> Option<Dec> {
            if is_zero(&divisor.unscaled) {
                return None;
            }
            let preferred = self.scale - divisor.scale;
            if self.compare_magnitude(divisor) == Ordering::Less {
                return Some(Dec {
                    unscaled: BigInt::from(0u8),
                    scale: preferred,
                });
            }
            let mut result = self.divide(divisor, Rounding::Down)?;
            if result.scale < 0 {
                let rest = self.subtract(&result.exact_multiply(divisor));
                if rest.compare_magnitude(divisor) != Ordering::Less {
                    return None;
                }
            } else if result.scale > 0 {
                // `setScale(0, DOWN)`.
                result = Dec {
                    unscaled: &result.unscaled / pow10(result.scale as u64),
                    scale: 0,
                };
            }
            let room = PRECISION as i64 - precision(&result.unscaled) as i64;
            if preferred > result.scale && room > 0 {
                let scale = result.scale + room.min(preferred - result.scale);
                return Some(Dec {
                    unscaled: result.at_scale(scale),
                    scale,
                });
            }
            Some(result.strip_to(preferred))
        }

        /// `remainder(divisor, DECIMAL128)`.
        pub(super) fn remainder(&self, divisor: &Dec) -> Option<Dec> {
            let q = self.divide_to_integral_value(divisor)?;
            Some(self.subtract(&q.exact_multiply(divisor)))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn dec(text: &str) -> Dec {
            Dec::parse(text).expect("a decimal")
        }

        /// `BigDecimal.toString()`'s digits and scale, as `unscaledE-scale`.
        fn show(d: Option<Dec>) -> String {
            d.map_or_else(|| "throws".to_string(), |d| d.scientific())
        }

        /// Each row's answer is `jshell` arithmetic on `java.math.BigDecimal`
        /// (JDK 25), written as `unscaledE-scale`.
        #[test]
        fn the_operations_keep_javas_scale_and_precision() {
            let one_third = format!("{}E-34", "3".repeat(34));
            let two_thirds = format!("{}7E-34", "6".repeat(33));
            for (got, want) in [
                (show(Dec::value_of(2.0)), "20E-1"),
                (show(Dec::value_of(-0.0)), "0E-1"),
                (show(Dec::value_of(1e20)), "10E19"),
                (
                    show(Dec::value_of(1.2345678901234568e29)),
                    "12345678901234568E13",
                ),
                (show(Dec::value_of(f64::NAN)), "throws"),
                (show(Dec::value_of(f64::INFINITY)), "throws"),
                (show(Some(dec("1.5").add(&dec("1.0")))), "25E-1"),
                (show(Some(dec("1.5").subtract(&dec("0.5")))), "10E-1"),
                (show(Some(dec("1.5").multiply(&dec("2.0")))), "300E-2"),
                (show(Some(dec("1.5").multiply(&dec("1.25")))), "1875E-3"),
                (
                    show(dec("1.0").divide(&dec("3.0"), Rounding::HalfEven)),
                    one_third.as_str(),
                ),
                (
                    show(dec("2.0").divide(&dec("3.0"), Rounding::HalfEven)),
                    two_thirds.as_str(),
                ),
                (
                    show(dec("1.5").divide(&dec("2.0"), Rounding::HalfEven)),
                    "75E-2",
                ),
                (
                    show(dec("100").divide(&dec("4"), Rounding::HalfEven)),
                    "25E0",
                ),
                (
                    show(dec("1.00").divide(&dec("1"), Rounding::HalfEven)),
                    "100E-2",
                ),
                (
                    show(dec("0.0").divide(&dec("3"), Rounding::HalfEven)),
                    "0E-1",
                ),
                (
                    show(dec("1.5").divide(&dec("0.0"), Rounding::HalfEven)),
                    "throws",
                ),
                (show(dec("5.5").remainder(&dec("2.0"))), "15E-1"),
                (show(dec("-5.5").remainder(&dec("2.0"))), "-15E-1"),
                (show(dec("1.5").remainder(&dec("2.0"))), "15E-1"),
                (show(dec("6.0").remainder(&dec("2.0"))), "0E-1"),
                (show(dec("1.5").remainder(&dec("0.0"))), "throws"),
            ] {
                assert_eq!(got, want);
            }
            // HALF_EVEN at the 35th digit: ...5 exactly rounds to even.
            let tie = Dec {
                unscaled: format!("{}5", "1".repeat(34)).parse().unwrap(),
                scale: 0,
            };
            assert_eq!(
                tie.multiply(&dec("1")).scientific(),
                format!("{}2E1", "1".repeat(33))
            );
            // Rounding up to 10^34 drops one digit.
            assert_eq!(
                dec(&"9".repeat(35)).multiply(&dec("1")).scientific(),
                format!("1{}E2", "0".repeat(33))
            );
        }
    }
}

macro_rules! arithmetic_function {
    ($name:ident, $op:expr) => {
        fn $name(state: &State, left: MjValue, right: MjValue) -> Result<MjValue, MjError> {
            $op.apply(state, &left, &right)
        }
    };
}

arithmetic_function!(kestra_add, ArithOp::Add);
arithmetic_function!(kestra_sub, ArithOp::Sub);
arithmetic_function!(kestra_mul, ArithOp::Mul);
arithmetic_function!(kestra_div, ArithOp::Div);
arithmetic_function!(kestra_mod, ArithOp::Mod);

/// Registers the five functions.
pub(super) fn register(env: &mut Environment<'static>) {
    env.add_function("kestra_add", kestra_add);
    env.add_function("kestra_sub", kestra_sub);
    env.add_function("kestra_mul", kestra_mul);
    env.add_function("kestra_div", kestra_div);
    env.add_function("kestra_mod", kestra_mod);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runners::env::{render_str, RenderContext};
    use serde_json::json;

    fn render_in(template: &str, ctx: serde_json::Value) -> Result<String, String> {
        // Through the deserialiser, as the oracle and a worker parse a
        // context: it keeps an integer past `u64` as an `i128`/`u128`
        // (`from_json_object`, over `serde_json::Value`, makes it an f64).
        let cx = RenderContext::from_json_str(&ctx.to_string()).expect("a JSON object");
        render_str(template, &cx).map_err(|e| e.to_string())
    }

    fn render(template: &str) -> Result<String, String> {
        render_in(template, json!({}))
    }

    fn compiled(source: &str) -> String {
        compile_arithmetic(source).expect("no reserved assignment target")
    }

    /// The shape the pass writes, and minijinja's precedence kept: `*`
    /// inside `+`, a left-associative run, unary minus inside the operand,
    /// `~` below `+` (Pebble's `~` is 110, above `+`, so the two engines
    /// agree), a filter and a call as whole operands, `//` and `**` left
    /// alone.
    #[test]
    fn the_compiled_text_keeps_minijinjas_precedence() {
        for (source, want) in [
            ("{{ a + b }}", "{{ kestra_add( a , b) }}"),
            (
                "{{ a + b * c }}",
                "{{ kestra_add( a , kestra_mul( b , c)) }}",
            ),
            (
                "{{ a * b + c }}",
                "{{ kestra_add( kestra_mul( a , b) , c) }}",
            ),
            (
                "{{ a - b + c }}",
                "{{ kestra_add( kestra_sub( a , b ), c) }}",
            ),
            (
                "{{ a / b % c }}",
                "{{ kestra_mod( kestra_div( a , b ), c) }}",
            ),
            ("{{ -a / b }}", "{{ kestra_div( kestra_mul( -1,a) , b) }}"),
            ("{{ a - -b }}", "{{ kestra_sub( a , kestra_mul( -1,b)) }}"),
            ("{{ a ~ b + c }}", "{{ kestra_add( a ~ b , c) }}"),
            ("{{ a + b ~ c }}", "{{ kestra_add( a , b ~ c) }}"),
            (
                "{{ l | length + f(x * 2) }}",
                "{{ kestra_add( l | length , f( kestra_mul(x , 2))) }}",
            ),
            (
                "{{ (a + b) * c }}",
                "{{ kestra_mul( ( kestra_add(a , b)) , c) }}",
            ),
            (
                "{{ a * b // c * d }}",
                "{{ kestra_mul( kestra_mul( a , b) // c , d) }}",
            ),
            ("{{ a ** b + c }}", "{{ kestra_add( a ** b , c) }}"),
            // Task A8 (2026-10-29): unary minus is Pebble's `multiply(-1, x)`;
            // a number literal is left alone.
            ("{{ -a }}", "{{ kestra_mul( -1,a) }}"),
            ("{{ --a }}", "{{ kestra_mul( -1, kestra_mul(-1,a)) }}"),
            // The operand is Pebble's: the primary with its postfix.
            ("{{ -a.b }}", "{{ kestra_mul( -1,a.b) }}"),
            (
                "{{ -f(x * 2) | abs }}",
                "{{ kestra_mul( -1,f( kestra_mul(x , 2))) | abs }}",
            ),
            ("{{ -1 + -2.5 }}", "{{ kestra_add( -1 , -2.5) }}"),
            (
                "{% if a + 1 > 2 %}y{% endif %}",
                "{% if kestra_add( a , 1) > 2 %}y{% endif %}",
            ),
            ("{% set x = a * 2 %}", "{% set x = kestra_mul( a , 2) %}"),
            ("{{ 'a + b' }}", "{{ 'a + b' }}"),
            ("a + b {{ 1 }}", "a + b {{ 1 }}"),
        ] {
            assert_eq!(compiled(source), want, "{source}");
        }
    }

    /// The full front end (rewrite, comparisons, arithmetic) on the same
    /// shapes, rendered: each answer is Pebble's.
    #[test]
    fn precedence_and_statement_positions_render_as_pebble() {
        let ctx = json!({"a": 7, "b": 2, "c": 3, "l": [1, 2, 3]});
        for (template, want) in [
            ("{{ a + b * c }}", "13"),
            ("{{ a - b - c }}", "2"),
            ("{{ -a / b }}", "-3"),
            ("{{ -a }}", "-7"),
            ("{{ -'5' }}", "-5"),
            ("{{ -(a + 0.5) }}", "-7.5"),
            ("{{ -l[0] }}", "-1"),
            ("{{ -(l | length) * 2 }}", "-6"),
            // `(a ~ b) + c` in both engines, and a String operand makes `+`
            // a concatenation.
            ("{{ a ~ b + c }}", "723"),
            ("{{ l | length + a }}", "10"),
            // Pebble's `range` is inclusive.
            ("{{ range(1, b + 1) | length }}", "3"),
            ("{% if a / b == 3 %}y{% else %}n{% endif %}", "y"),
            ("{% set x = a / b %}{{ x }}", "3"),
            ("{% for i in l %}{{ i * a }};{% endfor %}", "7;14;21;"),
            ("{{ a / b > 3 }}", "false"),
            ("{{ (a + 0.0) / b }}", "3.5"),
        ] {
            assert_eq!(
                render_in(template, ctx.clone()).unwrap(),
                want,
                "{template}"
            );
        }
    }

    /// The tower's edge cases (spec §4): truncating division, Java's `%`
    /// sign rule, wrapping at both ends, `Long.MIN_VALUE / -1`, the Double
    /// branch, `toNumber` on strings, a BigInteger's `longValue()`.
    #[test]
    fn long_and_double_arithmetic_is_javas() {
        for (template, ctx, want) in [
            ("{{ 7 / 2 }}", json!({}), "3"),
            ("{{ -7 / 2 }}", json!({}), "-3"),
            ("{{ 7 / -2 }}", json!({}), "-3"),
            ("{{ 7 % -2 }}", json!({}), "1"),
            ("{{ -7 % 2 }}", json!({}), "-1"),
            (
                "{{ 9223372036854775807 + 1 }}",
                json!({}),
                "-9223372036854775808",
            ),
            (
                "{{ -9223372036854775807 - 2 }}",
                json!({}),
                "9223372036854775807",
            ),
            ("{{ 4611686018427387904 * 4 }}", json!({}), "0"),
            (
                "{{ (-9223372036854775807 - 1) / -1 }}",
                json!({}),
                "-9223372036854775808",
            ),
            ("{{ (-9223372036854775807 - 1) % -1 }}", json!({}), "0"),
            ("{{ 7.0 / 2 }}", json!({}), "3.5"),
            ("{{ 1.5 * 2 }}", json!({}), "3.0"),
            ("{{ 0.1 + 0.2 }}", json!({}), "0.30000000000000004"),
            ("{{ 1.0 / 0 }}", json!({}), "Infinity"),
            ("{{ 0.0 / 0 }}", json!({}), "NaN"),
            ("{{ 1.5 % 0 }}", json!({}), "NaN"),
            ("{{ '5' * 2 }}", json!({}), "10"),
            ("{{ '7' / 2 }}", json!({}), "3"),
            ("{{ '5' - 2 }}", json!({}), "3"),
            ("{{ '2.5' * 2 }}", json!({}), "5.0"),
            ("{{ '1e2' * 1 }}", json!({}), "100.0"),
            ("{{ ' 1.5d' * 2 }}", json!({}), "3.0"),
            (
                "{{ b + 1 }}",
                json!({"b": 99999999999999999999u128}),
                "7766279631452241920",
            ),
            (
                "{{ b * 2 }}",
                json!({"b": 99999999999999999999u128}),
                "-2914184810805067778",
            ),
            (
                "{{ b + 0.5 }}",
                json!({"b": 99999999999999999999u128}),
                "1.0E20",
            ),
            ("{{ a * 65536 }}", json!({"a": 65536}), "4294967296"),
        ] {
            assert_eq!(render_in(template, ctx).unwrap(), want, "{template}");
        }
    }

    /// A `BigInteger` past `u128` (a `kv()` value) goes through the same
    /// `longValue()`/`doubleValue()`, and two `BigInteger`s take the `int`
    /// branch.
    #[test]
    fn a_big_integer_takes_the_long_double_or_int_branch() {
        let digits = "340282366920938463463374607431768211457"; // 2^128 + 1
        let Some(Operand::BigInteger { low, double }) = big_integer_of_digits(digits) else {
            panic!("a BigInteger");
        };
        assert_eq!(low, 1);
        assert_eq!(double, 3.402823669209385e38);
        let Some(Operand::BigInteger { low, .. }) = big_integer_of_digits("-18446744073709551617")
        else {
            panic!("a BigInteger");
        };
        assert_eq!(low, -1);
        // Two BigIntegers: `intValue()` and 32-bit arithmetic.
        let (a, b) = (
            MjValue::from(18_446_744_073_709_551_618i128), // 2^64 + 2
            MjValue::from(18_446_744_073_709_551_619i128), // 2^64 + 3
        );
        assert_eq!(ArithOp::Mul.numeric(&a, &b).unwrap(), MjValue::from(6i64));
        assert!(ArithOp::Div
            .numeric(&a, &MjValue::from(18_446_744_073_709_551_616i128))
            .is_err());
    }

    /// String `+`, list `+` and list `-`, per A1's measurements.
    #[test]
    fn strings_and_lists_follow_operator_utils() {
        for (template, ctx, want) in [
            ("{{ 1 + 'a' }}", json!({}), "1a"),
            ("{{ 'a' + 1 }}", json!({}), "a1"),
            ("{{ 1 + '2' }}", json!({}), "12"),
            ("{{ 'a' + 'b' }}", json!({}), "ab"),
            ("{{ 1.5 + 'a' }}", json!({}), "1.5a"),
            ("{{ 'a' + n }}", json!({"n": null}), "anull"),
            ("{{ 'a' + [1, 2] }}", json!({}), "a[1, 2]"),
            ("{{ 'a' + true }}", json!({}), "atrue"),
            ("{{ 'a' + m }}", json!({"m": {"k": 1}}), "a{k=1}"),
            ("{{ [1] + 'a' }}", json!({}), "[1]a"),
            ("{{ [1] + 2 }}", json!({}), "[1,2]"),
            ("{{ [1] + [2] }}", json!({}), "[1,2]"),
            ("{{ [1] + m }}", json!({"m": {"k": 1}}), "[1,{\"k\":1}]"),
            ("{{ [1, 2] - 1 }}", json!({}), "[2]"),
            ("{{ [1, 2, 1] - 1 }}", json!({}), "[2,1]"),
            ("{{ [1, 2, 3] - [1, 3] }}", json!({}), "[2]"),
            ("{{ [1, 2, 1] - [1] }}", json!({}), "[2]"),
            // Java's `equals`: a Long is not a Double.
            ("{{ [1.0, 2] - 1 }}", json!({}), "[1.0,2]"),
            ("{{ ['a', 'b'] - 'a' }}", json!({}), "[\"b\"]"),
        ] {
            assert_eq!(render_in(template, ctx).unwrap(), want, "{template}");
        }
    }

    /// I1 (fix wave #5a): the list operators' egret-only bounds. Each refused
    /// template here would run for seconds to hours, or allocate gigabytes,
    /// without them (sizes in the comments); with them each fails before the
    /// work, so the whole test takes well under a second of operator time.
    #[test]
    fn the_list_operators_refuse_unbounded_work() {
        let started = std::time::Instant::now();
        let work = format!("compare or copy more than {MAX_LIST_OPERATOR_WORK} elements");
        let length = format!("more than {MAX_LIST_OPERATOR_LENGTH} elements;");
        for (template, ctx, cap) in [
            // `removeAll`: 10^12 comparisons, about 1.8 h.
            ("{{ ((0..999999) - (0..999999)) | length }}", json!({}), &work),
            // `containsAll`: 5 x 10^11 comparisons.
            ("{{ (0..999999) contains (0..999999) }}", json!({}), &work),
            // The same, reached from data a fixpoint render evaluates.
            (
                "{{ s }}",
                json!({"s": "{{ (0..999999) contains (0..999999) }}"}),
                &work,
            ),
            // Cheap calls, many of them: one budget for the whole render.
            // 2 x 10^9 comparisons (tens of seconds) without it.
            (
                "{% set big = 0..999999 %}{% for i in 0..1999 %}{{ big contains -1 }}{% endfor %}",
                json!({}),
                &work,
            ),
            (
                "{% set big = 0..999999 %}{% for i in 0..1999 %}{{ (big - -1) | length }}{% endfor %}",
                json!({}),
                &work,
            ),
            // List `+`: ten doublings of a million is 10^9 values (tens of GB).
            (
                "{% set l = 0..999999 %}{% set l = l + l %}{% set l = l + l %}{% set l = l + l %}\
                 {% set l = l + l %}{% set l = l + l %}{% set l = l + l %}{% set l = l + l %}\
                 {% set l = l + l %}{% set l = l + l %}{% set l = l + l %}{{ l | length }}",
                json!({}),
                &length,
            ),
            ("{{ ((0..999999) + 1) | length }}", json!({}), &length),
        ] {
            let err = render_in(template, ctx).expect_err(template);
            assert!(err.contains("egret refuses"), "{template}: {err}");
            assert!(err.contains(cap.as_str()), "{template}: {err}");
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(30),
            "the refusals took {:?}",
            started.elapsed()
        );
    }

    /// Below the bounds the list operators answer as before.
    #[test]
    fn the_list_operator_bounds_leave_ordinary_lists_alone() {
        for (template, want) in [
            ("{{ (0..2999) - (0..2998) }}", "[2999]"),
            ("{{ (0..2999) contains (0..2999) }}", "true"),
            ("{{ ((0..499999) + (0..499998)) | length }}", "999999"),
            ("{{ ((0..999998) + 1) | length }}", "1000000"),
            ("{{ ((0..999999) - 5) | length }}", "999999"),
        ] {
            assert_eq!(render(template).unwrap(), want, "{template}");
        }
        assert_eq!(pair_work(0, 7), 0);
        assert_eq!(pair_work(3, 0), 3);
        assert_eq!(pair_work(usize::MAX, usize::MAX), u64::MAX);
    }

    /// Every arithmetic probe Kestra refuses fails here with Kestra's own
    /// text (`oracle/error-class-pairs.txt` pairs the `arith-*` error-class
    /// entries with this test).
    #[test]
    fn an_operation_kestra_refuses_fails_with_kestras_text() {
        for (template, ctx, op) in [
            ("{{ 1 / 0 }}", json!({}), ArithOp::Div),
            ("{{ 1 / n }}", json!({"n": 0}), ArithOp::Div),
            ("{{ 7 % 0 }}", json!({}), ArithOp::Mod),
            ("{{ n + 1 }}", json!({"n": null}), ArithOp::Add),
            ("{{ 2 * n }}", json!({"n": null}), ArithOp::Mul),
            ("{{ true + 1 }}", json!({}), ArithOp::Add),
            ("{{ b * 2 }}", json!({"b": true}), ArithOp::Mul),
            ("{{ 'abc' * 2 }}", json!({}), ArithOp::Mul),
            ("{{ 2 + [1] }}", json!({}), ArithOp::Add),
            ("{{ [1] * 2 }}", json!({}), ArithOp::Mul),
            ("{{ m + 1 }}", json!({"m": {"a": 1}}), ArithOp::Add),
            (
                "{{ m + n }}",
                json!({"m": {"a": 1}, "n": {"b": 2}}),
                ArithOp::Add,
            ),
            ("{{ m - 1 }}", json!({"m": {"a": 1}}), ArithOp::Sub),
            ("{{ '1.5x' * 2 }}", json!({}), ArithOp::Mul),
            ("{{ '99999999999999999999' * 1 }}", json!({}), ArithOp::Mul),
            ("{{ 1 + (x if false) }}", json!({}), ArithOp::Add),
        ] {
            let error = render_in(template, ctx).unwrap_err();
            assert!(error.contains(op.kestra_refusal()), "{template}: {error}");
        }
    }

    /// A strict undefined operand raises minijinja's undefined error, as a
    /// missing variable fails Kestra's strict render, before the operator.
    #[test]
    fn an_undefined_operand_is_an_undefined_error() {
        for template in [
            "{{ missing + 1 }}",
            "{{ 1 * missing.x }}",
            "{{ 'a' + missing }}",
        ] {
            let error = render(template).unwrap_err();
            assert!(error.contains("undefined"), "{template}: {error}");
        }
    }

    /// Task A4 (spec D3): a `BigDecimal` operand makes both operands
    /// `BigDecimal.valueOf(x.doubleValue())` and the result a `BigDecimal`,
    /// at `DECIMAL128` with Java's preferred scale. Every answer is A1's
    /// golden or `jshell`'s.
    #[test]
    fn big_decimal_arithmetic_is_javas() {
        for (template, want) in [
            ("{{ fromIon(ion='1.50') + 1 }}", "2.5"),
            ("{{ fromIon(ion='1.5') - 0.5 }}", "1.0"),
            ("{{ fromIon(ion='1.50') * 2 }}", "3.00"),
            ("{{ fromIon(ion='1.5') * fromIon(ion='1.25') }}", "1.875"),
            (
                "{{ fromIon(ion='1.0') / 3 }}",
                "0.3333333333333333333333333333333333",
            ),
            (
                "{{ fromIon(ion='2.0') / 3 }}",
                "0.6666666666666666666666666666666667",
            ),
            ("{{ fromIon(ion='1.5') / 2 }}", "0.75"),
            ("{{ fromIon(ion='5.5') % 2 }}", "1.5"),
            ("{{ 1 - fromIon(ion='1.5') }}", "-0.5"),
            // Task A8 (2026-10-29): unary minus is `multiply(-1, x)`, so the
            // operand goes through `valueOf(doubleValue())` like any other.
            ("{{ -fromIon(ion='1.50') }}", "-1.50"),
            // A4's regression: a decimal held in a variable.
            ("{% set d = fromIon(ion='1.50') %}{{ -d }}", "-1.50"),
            ("{{ -('1.10' | number(type='BIGDECIMAL')) }}", "-1.10"),
            (
                "{{ '123456789012345678901234567890.5' | number(type='BIGDECIMAL') + 1 }}",
                "123456789012345680000000000001.0",
            ),
            ("{{ '1.10' | number(type='BIGDECIMAL') * 3 }}", "3.30"),
            (
                "{{ (fromIon(ion='1.5') * 2) | className }}",
                "java.math.BigDecimal",
            ),
            // `valueOf(100.0)` is `100.0`, so the scale is 2; a String
            // operand concatenates `toString()`, which is `1E+2`.
            ("{{ fromIon(ion='1d2') * 1 }}", "100.00"),
            ("{{ (fromIon(ion='1d2') * 1) | toJson }}", "100.00"),
            ("{{ 'x' + fromIon(ion='1d2') }}", "x1E+2"),
        ] {
            assert_eq!(render(template).unwrap(), want, "{template}");
        }
        for (template, op) in [
            ("{{ fromIon(ion='1.5') / 0 }}", ArithOp::Div),
            ("{{ fromIon(ion='1.5') % 0 }}", ArithOp::Mod),
            ("{{ '1.5' | number(type='BIGDECIMAL') / 0 }}", ArithOp::Div),
            // `BigDecimal.valueOf(Infinity)` throws.
            ("{{ fromIon(ion='1.5') + 1.0 / 0 }}", ArithOp::Add),
        ] {
            let error = render(template).unwrap_err();
            assert!(error.contains(op.kestra_refusal()), "{template}: {error}");
        }
    }

    #[test]
    fn java_parse_double_takes_javas_decimal_forms_only() {
        assert_eq!(java_parse_double("1.5"), Some(1.5));
        assert_eq!(java_parse_double(" -1.5e2 "), Some(-150.0));
        assert_eq!(java_parse_double(".5"), Some(0.5));
        assert_eq!(java_parse_double("5."), Some(5.0));
        assert_eq!(java_parse_double("1.5f"), Some(1.5));
        assert_eq!(java_parse_double("1e"), None);
        assert_eq!(java_parse_double("."), None);
        assert_eq!(java_parse_double("inf.0"), None);
        assert_eq!(java_parse_double("1.5.2"), None);
    }
}
