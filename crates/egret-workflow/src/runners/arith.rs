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
//! **A `BigDecimal` operand is not computed yet**: Task A4 ports
//! `bigDecimalOperation`. Until then such an operation fails, naming the
//! operator in the words minijinja's own refusal used (`tried to use +
//! operator on unsupported types`), as it did before this pass (W9c spec
//! §15).

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

    /// The operator as written.
    fn symbol(self) -> &'static str {
        match self {
            ArithOp::Add => "+",
            ArithOp::Sub => "-",
            ArithOp::Mul => "*",
            ArithOp::Div => "/",
            ArithOp::Mod => "%",
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
                    items.extend(list_items(right));
                } else {
                    items.push(right.clone());
                }
                Ok(MjValue::from(items))
            }
            ArithOp::Sub if is_list(left) => {
                let mut items = list_items(left);
                if is_list(right) {
                    let remove = list_items(right);
                    items.retain(|item| !remove.iter().any(|r| java_equals(item, r)));
                } else if let Some(at) = items.iter().position(|item| java_equals(right, item)) {
                    items.remove(at);
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
        if matches!(a, Operand::BigDecimal) || matches!(b, Operand::BigDecimal) {
            return Err(MjError::new(
                ErrorKind::InvalidOperation,
                format!(
                    "tried to use {} operator on unsupported types: egret does not \
                     compute on a java.math.BigDecimal",
                    self.symbol()
                ),
            ));
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
    BigDecimal,
}

impl Operand {
    fn double_value(self) -> f64 {
        match self {
            Operand::Long(v) => v as f64,
            Operand::Double(v) => v,
            Operand::BigInteger { double, .. } => double,
            Operand::BigDecimal => unreachable!("a BigDecimal is handled before"),
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
            Operand::BigDecimal => unreachable!("a BigDecimal is handled before"),
        }
    }
}

/// `OperatorUtils.toNumber`; `None` is the exception it throws (or that
/// `parseDouble`/`parseLong` throw).
fn to_number(value: &MjValue) -> Option<Operand> {
    if let Some(class) = super::filters::big_number_class(value) {
        if class == "java.math.BigDecimal" {
            return Some(Operand::BigDecimal);
        }
        let digits = super::filters::big_number_plain(value)?;
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

/// `String.valueOf(Object)`: `"null"` for a null (a silent undefined is
/// egret's spelling of that null), else the value's Java `toString()`.
fn java_string_value_of(value: &MjValue) -> String {
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
fn java_equals(a: &MjValue, b: &MjValue) -> bool {
    let (ca, cb) = (
        super::filters::big_number_class(a),
        super::filters::big_number_class(b),
    );
    if ca.is_some() || cb.is_some() {
        return ca == cb
            && super::filters::big_number_plain(a) == super::filters::big_number_plain(b);
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
            ("{{ -a / b }}", "{{ kestra_div( -a , b) }}"),
            ("{{ a - -b }}", "{{ kestra_sub( a , -b) }}"),
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
            ("{{ -a }}", "{{ -a }}"),
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

    /// Until Task A4, a `BigDecimal` operand is refused naming the operator.
    #[test]
    fn a_big_decimal_operand_is_refused_until_a4() {
        let error = render("{{ '1.5' | number(type='BIGDECIMAL') * 2 }}").unwrap_err();
        assert!(error.contains("tried to use * operator"), "{error}");
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
