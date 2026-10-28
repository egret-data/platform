//! W9c Task 4 fix round 1 (the user's ruling, 2026-10-07, option A
//! 2026-10-08): **comparisons on exact numbers compare exactly.**
//!
//! `kv()` hands a stored decimal or an integer past `u128` to a template as a
//! `filters::BigNumber` (`java.math.BigDecimal` / `java.math.BigInteger`),
//! and `number(type='BIGDECIMAL')` returns the same carrier. minijinja cannot
//! compare that carrier against a number, and this is a property of the
//! engine, not a choice (minijinja 2.24.0, `value/mod.rs`):
//!
//! - `Ord for Value` compares `kind()` FIRST and returns on a difference, and
//!   `ValueKind::Plain` sorts above `ValueKind::Number` -- so `kv('d') > 100`
//!   was `true` for every decimal;
//! - `PartialEq` against a number goes through `ops::coerce`, which has no
//!   object arm -- so `kv('d') == 1.5` was `false`;
//! - `Object::custom_cmp` is consulted only when both sides are objects of the
//!   SAME type, and two plain objects otherwise compare as text -- so
//!   `-1.5 > -2.5` was `false`.
//!
//! There is no operator hook and no numeric `ValueRepr` that can print `1.50`.
//! So the comparison operators never reach minijinja's own `==`/`<`/`in`:
//! [`compile_comparisons`] (the rewriter's second pass) turns every
//! `a OP b` into a call to one of the functions here, and each of them
//!
//! - answers **exactly** when a `BigNumber` meets a number or another
//!   `BigNumber` (see [`exact_cmp`]), and
//! - otherwise answers `==`, `!=`, `in` and `not in` **with minijinja's own
//!   `PartialEq`**, after the same undefined checks, in the same order,
//!   raising the same error the VM instruction would have raised.
//!
//! The second half is what makes the pass safe to apply to every equality and
//! containment test in every template: for any pair without a `BigNumber` the
//! function and the instruction are the same code.
//! `tests::equality_and_containment_agree_with_minijinja_on_every_other_pair`
//! is the differential that holds it to that.
//!
//! **`<`, `<=`, `>` and `>=` are Kestra's, not minijinja's** (package #5a,
//! spec D1): two strings or two numbers are ordered, a null answers `false`,
//! and every other pair fails with Kestra's own text -- see [`relational`].
//! minijinja's total order across kinds (`'2' > 10` was `true`) is gone. A
//! chain `a < b < c` is no longer minijinja's `a < b and b < c`: the pass
//! compiles it left-associatively, as Pebble parses it, so the outer
//! comparison sees a boolean and fails.
//!
//! **A float is compared by its exact binary value** (the ruling, 2026-10-08).
//! `0.1` the float is `0.1000000000000000055511151231257827…`, so
//! `kv('d') == 0.1` is `false` for a stored decimal `0.1`. Pebble 4.1.2
//! differs: `OperatorUtils.wideningConversionBinaryComparison` compares any
//! two `Number`s by `doubleValue()` (read from the pinned jar's bytecode), so
//! there the decimal is rounded to a double first and the two are equal; and
//! two decimals that differ past the 17th significant digit are equal in
//! Pebble and not here. Spec §15 records both.

use std::cmp::Ordering;

use minijinja::value::{ObjectRepr, StringInput, Value as MjValue};
use minijinja::{Environment, Error as MjError, ErrorKind, State};

pub(super) use super::cmp_rewrite::compile_comparisons;

// ---------------------------------------------------------------------------
// Exact numbers
// ---------------------------------------------------------------------------

/// A number as an exact decimal: `digits × 10^exp`, or one of the three
/// non-finite floats.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Exact {
    /// `digits` has no leading and no trailing zero; zero is the empty digit
    /// string with `negative == false`, so `-0.0`, `0`, `0.00` are one value.
    Finite {
        negative: bool,
        digits: Vec<u8>,
        exp: i64,
    },
    PosInf,
    NegInf,
    /// `f64::total_cmp`'s rule, which is minijinja's `cmp_f64` for NaN: a
    /// NaN with the sign bit clear sorts above `+inf`, one with it set below
    /// `-inf`.
    NaN {
        negative: bool,
    },
}

impl Exact {
    /// Builds a finite value from a sign, ASCII digits and an exponent,
    /// normalising leading and trailing zeros away.
    fn finite(negative: bool, digits: &[u8], exp: i64) -> Exact {
        let start = digits.iter().position(|d| *d != b'0');
        let Some(start) = start else {
            return Exact::Finite {
                negative: false,
                digits: Vec::new(),
                exp: 0,
            };
        };
        let end = digits
            .iter()
            .rposition(|d| *d != b'0')
            .expect("a non-zero digit exists")
            + 1;
        Exact::Finite {
            negative,
            digits: digits[start..end].to_vec(),
            exp: exp + (digits.len() - end) as i64,
        }
    }

    /// `-?[0-9]+(\.[0-9]+)?` -- `BigDecimal.toPlainString()` and
    /// `BigInteger.toString()`, which is what a `BigNumber` keeps as its text,
    /// and also what `i128`/`u128` `to_string` writes.
    fn parse_plain(text: &str) -> Option<Exact> {
        let (negative, body) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let (int_part, fraction) = match body.split_once('.') {
            Some((i, f)) => (i, f),
            None => (body, ""),
        };
        if int_part.is_empty()
            || !int_part.bytes().all(|b| b.is_ascii_digit())
            || !fraction.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        let mut digits = Vec::with_capacity(int_part.len() + fraction.len());
        digits.extend_from_slice(int_part.as_bytes());
        digits.extend_from_slice(fraction.as_bytes());
        Some(Exact::finite(negative, &digits, -(fraction.len() as i64)))
    }

    /// The exact value of an `f64`.
    ///
    /// Every finite `f64` is `m × 2^e`, a finite decimal of at most 767
    /// significant digits. `{:.800e}` is Rust's EXACT formatting mode
    /// (`core::fmt` falls back to Dragon4 when Grisu cannot prove a digit), so
    /// 801 digits print the value itself, padded with zeros, never a rounding
    /// of it. `0.1` comes out as `1.000000000000000055511151231257827…e-1`.
    fn of_f64(value: f64) -> Exact {
        if value.is_nan() {
            return Exact::NaN {
                negative: value.is_sign_negative(),
            };
        }
        if value.is_infinite() {
            return if value > 0.0 {
                Exact::PosInf
            } else {
                Exact::NegInf
            };
        }
        let text = format!("{:.800e}", value.abs());
        let (mantissa, exponent) = text.split_once('e').expect("`{:e}` writes an `e`");
        let exponent: i64 = exponent.parse().expect("`{:e}` writes an integer exponent");
        let (int_part, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
        let mut digits = Vec::with_capacity(int_part.len() + fraction.len());
        digits.extend_from_slice(int_part.as_bytes());
        digits.extend_from_slice(fraction.as_bytes());
        Exact::finite(
            value.is_sign_negative(),
            &digits,
            exponent - fraction.len() as i64,
        )
    }

    /// The total order over exact values.
    fn cmp(&self, other: &Exact) -> Ordering {
        fn rank(e: &Exact) -> i8 {
            match e {
                Exact::NaN { negative: true } => -2,
                Exact::NegInf => -1,
                Exact::Finite { .. } => 0,
                Exact::PosInf => 1,
                Exact::NaN { negative: false } => 2,
            }
        }
        match (self, other) {
            (
                Exact::Finite {
                    negative: an,
                    digits: ad,
                    exp: ae,
                },
                Exact::Finite {
                    negative: bn,
                    digits: bd,
                    exp: be,
                },
            ) => {
                // Sign first; zero is non-negative and has no digits.
                let sign = |neg: bool, d: &[u8]| -> i8 {
                    if d.is_empty() {
                        0
                    } else if neg {
                        -1
                    } else {
                        1
                    }
                };
                let (sa, sb) = (sign(*an, ad), sign(*bn, bd));
                if sa != sb || sa == 0 {
                    return sa.cmp(&sb);
                }
                let magnitude = magnitude_cmp(ad, *ae, bd, *be);
                if sa < 0 {
                    magnitude.reverse()
                } else {
                    magnitude
                }
            }
            _ => rank(self).cmp(&rank(other)),
        }
    }
}

/// Compares two non-zero magnitudes `ad × 10^ae` and `bd × 10^be`, each with
/// no leading or trailing zero digit.
fn magnitude_cmp(ad: &[u8], ae: i64, bd: &[u8], be: i64) -> Ordering {
    // The position of the most significant digit decides first.
    let a_top = ad.len() as i64 + ae;
    let b_top = bd.len() as i64 + be;
    if a_top != b_top {
        return a_top.cmp(&b_top);
    }
    // Same top position: the digit strings line up from the left. With no
    // trailing zeros, equal prefixes mean the longer one has more non-zero
    // digits below, so it is the larger.
    for (x, y) in ad.iter().zip(bd.iter()) {
        if x != y {
            return x.cmp(y);
        }
    }
    ad.len().cmp(&bd.len())
}

/// The exact value of a number or a `BigNumber`; `None` for anything else.
fn exact_of(value: &MjValue) -> Option<Exact> {
    if let Some(text) = super::filters::big_number_plain(value) {
        return Exact::parse_plain(text);
    }
    if !value.is_number() {
        return None;
    }
    if value.is_integer() {
        if let Ok(i) = i128::try_from(value.clone()) {
            return Exact::parse_plain(&i.to_string());
        }
        if let Ok(u) = u128::try_from(value.clone()) {
            return Exact::parse_plain(&u.to_string());
        }
        return None;
    }
    f64::try_from(value.clone()).ok().map(Exact::of_f64)
}

/// The exact ordering of `a` and `b` when **one of them is a `BigNumber`** and
/// the other is a number or a `BigNumber`; `None` otherwise, which means
/// "minijinja's own answer stands".
pub(super) fn exact_cmp(a: &MjValue, b: &MjValue) -> Option<Ordering> {
    if !super::filters::is_big_number(a) && !super::filters::is_big_number(b) {
        return None;
    }
    let (x, y) = (exact_of(a)?, exact_of(b)?);
    Some(x.cmp(&y))
}

/// `a == b`: exact for a `BigNumber` pair, minijinja's `PartialEq` otherwise.
pub(super) fn values_eq(a: &MjValue, b: &MjValue) -> bool {
    // A `BigNumber` is finite, so a NaN on the other side is never `Equal`
    // here either -- as in minijinja's `F64 == F64`.
    match exact_cmp(a, b) {
        Some(ordering) => ordering == Ordering::Equal,
        None => a == b,
    }
}

/// The ordering: exact for a `BigNumber` pair, minijinja's `Ord` otherwise.
pub(super) fn values_cmp(a: &MjValue, b: &MjValue) -> Ordering {
    exact_cmp(a, b).unwrap_or_else(|| a.cmp(b))
}

// ---------------------------------------------------------------------------
// The functions the rewriter's second pass emits calls to
// ---------------------------------------------------------------------------

/// The VM's `assert_value_not_undefined` / `assert_iterable` (the two are the
/// same test): under `UndefinedBehavior::Strict` a plain undefined raises
/// `UndefinedError`, and the SILENT undefined of a missing `else` does not.
///
/// minijinja does not apply strictness to function arguments, and a function
/// cannot see which of the two undefineds it was handed, so this asks the one
/// public API that applies exactly that check, `StringInput::new`. Its error
/// is `Error::from(ErrorKind::UndefinedError)`, the very value the VM raises,
/// and the VM attaches the same location to it either way.
pub(super) fn assert_defined(state: &State, value: &MjValue) -> Result<(), MjError> {
    if !value.is_undefined() {
        return Ok(());
    }
    match StringInput::new(state, value) {
        Err(err) if err.kind() == ErrorKind::UndefinedError => Err(err),
        _ => Ok(()),
    }
}

/// A comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    In,
    NotIn,
}

impl CmpOp {
    /// The function a comparison becomes.
    pub(super) fn function(self) -> &'static str {
        match self {
            CmpOp::Eq => "kestra_eq",
            CmpOp::Ne => "kestra_ne",
            CmpOp::Lt => "kestra_lt",
            CmpOp::Le => "kestra_le",
            CmpOp::Gt => "kestra_gt",
            CmpOp::Ge => "kestra_ge",
            CmpOp::In => "kestra_in",
            CmpOp::NotIn => "kestra_not_in",
        }
    }

    /// One comparison, with the VM's checks in the VM's order.
    ///
    /// `Eq`/`Ne` (`op_binop!`): the left operand, then the right, then
    /// minijinja's `PartialEq` (exact for a `BigNumber`). `In`
    /// (`Instruction::In`): the CONTAINER first (`assert_iterable`), then the
    /// needle, then `ops::contains`. `Lt`..`Ge`: the left operand, then the
    /// right, then [`relational`] -- Kestra's rule, not minijinja's total
    /// order (package #5a, D1).
    fn apply(self, state: &State, left: &MjValue, right: &MjValue) -> Result<bool, MjError> {
        match self {
            CmpOp::In | CmpOp::NotIn => {
                assert_defined(state, right)?;
                assert_defined(state, left)?;
                let found = contains(right, left)?;
                Ok(if self == CmpOp::In { found } else { !found })
            }
            CmpOp::Eq | CmpOp::Ne => {
                assert_defined(state, left)?;
                assert_defined(state, right)?;
                let equal = values_eq(left, right);
                Ok(if self == CmpOp::Eq { equal } else { !equal })
            }
            CmpOp::Lt | CmpOp::Le | CmpOp::Gt | CmpOp::Ge => {
                assert_defined(state, left)?;
                assert_defined(state, right)?;
                match relational(left, right) {
                    Relational::Ordered(ordering) => Ok(match self {
                        CmpOp::Lt => ordering == Ordering::Less,
                        CmpOp::Le => ordering != Ordering::Greater,
                        CmpOp::Gt => ordering == Ordering::Greater,
                        _ => ordering != Ordering::Less,
                    }),
                    Relational::Unordered => Ok(false),
                    Relational::Refused => Err(MjError::new(
                        ErrorKind::InvalidOperation,
                        self.kestra_refusal(),
                    )),
                }
            }
        }
    }

    /// The text Kestra's `LessThanExpression` & co. wrap `OperatorUtils`'
    /// failure in, verbatim -- `comparisonPebb` included: that is how
    /// `io.kestra.core.runners.pebble.expression.LessThanExpression` spells
    /// it, and the oracle's goldens (`lt-mixed`, `cmp-*`, `chain-*`) hold it.
    fn kestra_refusal(self) -> &'static str {
        match self {
            CmpOp::Lt => "Could not perform less than comparisonPebb",
            CmpOp::Le => "Could not perform less than or equals comparison",
            CmpOp::Gt => "Could not perform greater than comparison",
            CmpOp::Ge => "Could not perform greater than or equals comparison",
            _ => unreachable!("only a relational operator is refused"),
        }
    }
}

/// What a relational operator (`<`, `<=`, `>`, `>=`) makes of a pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Relational {
    Ordered(Ordering),
    /// The answer is `false` whatever the operator: a null on either side,
    /// or a NaN.
    Unordered,
    /// Kestra fails the render.
    Refused,
}

/// Kestra's relational comparison (`io.kestra.core.runners.pebble.expression
/// .LessThanExpression` and its three siblings, over Pebble 4.1.2's
/// `OperatorUtils.wideningConversionBinaryComparison`, read with `javap -c`):
///
/// 1. two strings compare with `String.compareTo`, which orders UTF-16 code
///    units (so a supplementary character sorts below U+E000..U+FFFF, where
///    Rust's `str` order would put it above);
/// 2. otherwise a null on either side answers `false` -- a silent undefined
///    (a missing `else`) is egret's spelling of that null;
/// 3. otherwise two numbers compare numerically, and any comparison with a
///    NaN is `false` (`dcmpl`/`dcmpg`);
/// 4. anything else -- a string against a number, a boolean, a list, a map,
///    and so the boolean a left-associative chain `a < b < c` hands its outer
///    comparison -- fails (`ClassCastException`, wrapped as `Could not perform
///    … comparison`).
///
/// **Numbers compare as egret has compared them since W9c Task 4**, exactly
/// for a `BigNumber` and with minijinja's numeric order otherwise; Pebble
/// compares every pair by `doubleValue()`. The two differ only past 2^53 or
/// past a double's 17th digit (spec §15 of W9c records it).
fn relational(left: &MjValue, right: &MjValue) -> Relational {
    if let (Some(a), Some(b)) = (left.as_str(), right.as_str()) {
        return Relational::Ordered(a.encode_utf16().cmp(b.encode_utf16()));
    }
    let is_null = |v: &MjValue| v.is_none() || v.is_undefined();
    if is_null(left) || is_null(right) {
        return Relational::Unordered;
    }
    let is_numeric = |v: &MjValue| v.is_number() || super::filters::is_big_number(v);
    if !is_numeric(left) || !is_numeric(right) {
        return Relational::Refused;
    }
    let is_nan = |v: &MjValue| {
        v.is_number() && !v.is_integer() && f64::try_from(v.clone()).is_ok_and(f64::is_nan)
    };
    if is_nan(left) || is_nan(right) {
        return Relational::Unordered;
    }
    Relational::Ordered(values_cmp(left, right))
}

/// minijinja's `value::ops::contains`, line for line, with the element test
/// going through [`values_eq`] so `kv('d') in [1.5]` is exact.
fn contains(container: &MjValue, needle: &MjValue) -> Result<bool, MjError> {
    if container.is_undefined() {
        return Ok(false);
    }
    if let Some(haystack) = container.as_str() {
        return Ok(match needle.as_str() {
            Some(s) => haystack.contains(s),
            None => haystack.contains(&needle.to_string()),
        });
    }
    if let Some(obj) = container.as_object() {
        return Ok(match obj.repr() {
            ObjectRepr::Plain => false,
            ObjectRepr::Map => obj.get_value(needle).is_some(),
            ObjectRepr::Seq | ObjectRepr::Iterable => obj
                .try_iter()
                .into_iter()
                .flatten()
                .any(|item| values_eq(&item, needle)),
            // `ObjectRepr` is `#[non_exhaustive]`; 2.24.0 has exactly the
            // four above.
            _ => false,
        });
    }
    Err(MjError::new(
        ErrorKind::InvalidOperation,
        "cannot perform a containment check on this value",
    ))
}

macro_rules! comparison_function {
    ($name:ident, $op:expr) => {
        fn $name(state: &State, left: MjValue, right: MjValue) -> Result<bool, MjError> {
            $op.apply(state, &left, &right)
        }
    };
}

comparison_function!(kestra_eq, CmpOp::Eq);
comparison_function!(kestra_ne, CmpOp::Ne);
comparison_function!(kestra_lt, CmpOp::Lt);
comparison_function!(kestra_le, CmpOp::Le);
comparison_function!(kestra_gt, CmpOp::Gt);
comparison_function!(kestra_ge, CmpOp::Ge);
comparison_function!(kestra_in, CmpOp::In);
comparison_function!(kestra_not_in, CmpOp::NotIn);

/// Registers the eight functions.
pub(super) fn register(env: &mut Environment<'static>) {
    env.add_function("kestra_eq", kestra_eq);
    env.add_function("kestra_ne", kestra_ne);
    env.add_function("kestra_lt", kestra_lt);
    env.add_function("kestra_le", kestra_le);
    env.add_function("kestra_gt", kestra_gt);
    env.add_function("kestra_ge", kestra_ge);
    env.add_function("kestra_in", kestra_in);
    env.add_function("kestra_not_in", kestra_not_in);
}

/// For the `sort` filter and `BigNumber::custom_cmp`: the exact order of two
/// `BigNumber` plain texts, or `None` if either does not parse.
pub(super) fn plain_text_cmp(a: &str, b: &str) -> Option<Ordering> {
    Some(Exact::parse_plain(a)?.cmp(&Exact::parse_plain(b)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runners::env::{environment, render_str, RenderContext};
    use serde_json::json;

    fn render(template: &str) -> Result<String, String> {
        render_str(template, &RenderContext::new()).map_err(|e| e.to_string())
    }

    /// The pass on a template with no reserved assignment target.
    fn compiled(source: &str) -> String {
        compile_comparisons(source).expect("no reserved assignment target")
    }

    /// One template, rendered twice by the same environment: once from the
    /// first pass's output alone (what egret ran before this round), once
    /// with the comparisons compiled. Errors compare as their full text.
    fn old_and_new(
        template: &str,
        context: &serde_json::Value,
    ) -> (Result<String, String>, Result<String, String>, String) {
        let env = environment();
        let first = super::super::rewrite::rewrite(template).expect("the first pass rewrites");
        let compiled = compile_comparisons(&first).expect("no reserved assignment target");
        // Through `RenderContext`, as a render does: `serde_json`'s
        // `arbitrary_precision` makes `from_serialize` hand over every number
        // as a one-key map.
        let ctx = RenderContext::from_json_object(context)
            .expect("a JSON object")
            .to_minijinja();
        let run = |src: &str| {
            env.template_from_str(src)
                .and_then(|t| t.render(&ctx))
                .map_err(|e| e.to_string())
        };
        (run(&first), run(&compiled), compiled)
    }

    fn context() -> serde_json::Value {
        json!({
            "ci": 3, "cf": 2.5, "cs": "b", "cl": [1, 2, "a"], "cm": {"a": 1, "k": "v"},
            "cn": null, "cbig": 18446744073709551615u64, "cneg": -7,
        })
    }

    /// Every operand kind a template can compare without a `BigNumber`.
    const OPERANDS: &[&str] = &[
        "0",
        "1",
        "-1",
        "2",
        "9223372036854775807",
        "18446744073709551615",
        "170141183460469231731687303715884105727",
        "9007199254740993",
        "1.5",
        "-2.5",
        "0.0",
        "-0.0",
        "1e300",
        "1e400",
        "(1e400 - 1e400)",
        "3.0",
        "'a'",
        "'b'",
        "''",
        "'1'",
        "'abc'",
        "none",
        "true",
        "false",
        "[]",
        "[1]",
        "[1, 2]",
        "['a']",
        "[none]",
        "{}",
        "{'a': 1}",
        "nothere",
        "(1 if false)",
        "ci",
        "cf",
        "cs",
        "cl",
        "cm",
        "cn",
        "cbig",
        "cneg",
        "cm.a",
        "cl[0]",
    ];

    const OPS: &[&str] = &["==", "!=", "<", "<=", ">", ">=", "in", "not in"];

    /// The operators that still answer as minijinja does.
    const EQUALITY_OPS: &[&str] = &["==", "!=", "in", "not in"];

    /// The operators that answer as Kestra does (D1).
    const RELATIONAL_OPS: &[&str] = &["<", "<=", ">", ">="];

    /// Kestra's refusal text for each relational operator.
    fn kestra_refusal(op: &str) -> &'static str {
        match op {
            "<" => "Could not perform less than comparisonPebb",
            "<=" => "Could not perform less than or equals comparison",
            ">" => "Could not perform greater than comparison",
            ">=" => "Could not perform greater than or equals comparison",
            _ => unreachable!(),
        }
    }

    /// What an [`OPERANDS`] entry is to a relational operator, written out by
    /// hand so the test does not ask the code under test.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Class {
        Number,
        NaN,
        Text,
        Null,
        Undefined,
        Other,
    }

    fn class(operand: &str) -> Class {
        match operand {
            "(1e400 - 1e400)" => Class::NaN,
            "'a'" | "'b'" | "''" | "'1'" | "'1.5'" | "'abc'" | "cs" => Class::Text,
            "none" | "cn" | "(1 if false)" => Class::Null,
            "nothere" => Class::Undefined,
            "true" | "false" | "[]" | "[1]" | "[1, 2]" | "['a']" | "[none]" | "{}" | "{'a': 1}"
            | "cl" | "cm" => Class::Other,
            _ => Class::Number,
        }
    }

    /// **The differential.** For every pair of [`OPERANDS`] under `==`, `!=`,
    /// `in` and `not in`, the compiled template answers -- or refuses, word
    /// for word -- exactly as minijinja's own operator did.
    #[test]
    fn equality_and_containment_agree_with_minijinja_on_every_other_pair() {
        let ctx = context();
        let mut checked = 0usize;
        let mut errors = 0usize;
        for a in OPERANDS {
            for b in OPERANDS {
                for op in EQUALITY_OPS {
                    let template = format!("{{{{ {a} {op} {b} }}}}");
                    let (old, new, compiled) = old_and_new(&template, &ctx);
                    assert!(compiled.contains("kestra_"), "{template} was not compiled");
                    assert_eq!(new, old, "{template} (compiled: {compiled})");
                    checked += 1;
                    errors += usize::from(old.is_err());
                }
            }
        }
        // Both halves are populated: answers and refusals.
        assert_eq!(
            checked,
            OPERANDS.len() * OPERANDS.len() * EQUALITY_OPS.len()
        );
        assert!(
            errors > 100 && errors < checked / 2,
            "{errors} of {checked} refused"
        );
    }

    /// **D1, over every pair of [`OPERANDS`].** A relational operator answers
    /// as Kestra's `LessThanExpression` & co.: an undefined operand raises
    /// minijinja's own error as before, two strings and two numbers are
    /// ordered as before, a null on either side or a NaN answers `false`, and
    /// every other pair fails with Kestra's text.
    #[test]
    fn a_relational_operator_answers_as_kestra_on_every_pair() {
        let ctx = context();
        let (mut ordered, mut unordered, mut refused) = (0usize, 0usize, 0usize);
        for a in OPERANDS {
            for b in OPERANDS {
                for op in RELATIONAL_OPS {
                    let template = format!("{{{{ {a} {op} {b} }}}}");
                    let (old, new, compiled) = old_and_new(&template, &ctx);
                    assert!(compiled.contains("kestra_"), "{template} was not compiled");
                    let (ca, cb) = (class(a), class(b));
                    if ca == Class::Undefined || cb == Class::Undefined {
                        assert_eq!(new, old, "{template}");
                        assert!(new.is_err(), "{template}");
                    } else if ca == Class::Text && cb == Class::Text
                        || ca == Class::Number && cb == Class::Number
                    {
                        assert_eq!(new, old, "{template}");
                        ordered += 1;
                    } else if ca == Class::Null
                        || cb == Class::Null
                        || matches!((ca, cb), (Class::NaN, Class::Number | Class::NaN))
                        || (ca, cb) == (Class::Number, Class::NaN)
                    {
                        assert_eq!(new.as_deref(), Ok("false"), "{template}");
                        unordered += 1;
                    } else {
                        let error = new.expect_err(&template);
                        assert!(error.contains(kestra_refusal(op)), "{template}: {error}");
                        refused += 1;
                    }
                }
            }
        }
        assert!(
            ordered > 500 && unordered > 300 && refused > 1000,
            "{ordered} ordered, {unordered} unordered, {refused} refused"
        );
    }

    /// **A chain is left-associative** (D1): every two-operator chain renders
    /// -- or fails, word for word -- as its parenthesised left-hand reading.
    #[test]
    fn a_chain_is_its_left_associative_reading_on_every_triple() {
        let ctx = context();
        let operands = [
            "1", "2", "1.5", "'a'", "none", "[1]", "nothere", "ci", "true",
        ];
        let mut checked = 0usize;
        for a in operands {
            for b in operands {
                for c in operands {
                    for op1 in OPS {
                        for op2 in ["<", "==", "in", ">=", "!="] {
                            let chain = format!("{{{{ {a} {op1} {b} {op2} {c} }}}}");
                            let grouped = format!("{{{{ ({a} {op1} {b}) {op2} {c} }}}}");
                            let (_, new, compiled) = old_and_new(&chain, &ctx);
                            let (_, want, _) = old_and_new(&grouped, &ctx);
                            assert_eq!(compiled.matches("kestra_").count(), 2, "{compiled}");
                            assert_eq!(new, want, "{chain} (compiled: {compiled})");
                            checked += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(checked, 9 * 9 * 9 * 8 * 5);
    }

    /// The oracle's comparison entries (`gt-mixed`, `lt-mixed`, `lte-mixed`,
    /// `gte-mixed`, `cmp-*`, `chain-*`), as unit tests: what Kestra refuses
    /// fails here with Kestra's own text, and what it answers is answered.
    /// `oracle/error-class-pairs.txt` pairs the refused ones with this test.
    #[test]
    fn a_comparison_kestra_refuses_fails_with_kestras_text() {
        for (template, ctx, op) in [
            ("{{ '2' > 10 }}", json!({}), ">"),
            ("{{ '2' < 10 }}", json!({}), "<"),
            ("{{ '2' <= 10 }}", json!({}), "<="),
            ("{{ '2' >= 10 }}", json!({}), ">="),
            ("{{ n < '5' }}", json!({"n": 3}), "<"),
            ("{{ a > b }}", json!({"a": 3, "b": "2"}), ">"),
            ("{{ true < 3 }}", json!({}), "<"),
            ("{{ false < true }}", json!({}), "<"),
            ("{{ [1] < [2] }}", json!({}), "<"),
            ("{{ m < n }}", json!({"m": {"a": 1}, "n": {"a": 2}}), "<"),
            ("{{ 1 < 2 < 3 }}", json!({}), "<"),
            ("{{ 3 < 2 < 1 }}", json!({}), "<"),
            ("{{ 1 == 1 < 2 }}", json!({}), "<"),
            // The corpus's one flow that writes it
            // (`valids/loopuntil-failed-flowable`).
            (
                "{{ outputs.loop.iterationCount < \"5\" }}",
                json!({"outputs": {"loop": {"iterationCount": 1}}}),
                "<",
            ),
        ] {
            let cx = RenderContext::from_json_object(&ctx).unwrap();
            let error = render_str(template, &cx).unwrap_err().to_string();
            assert!(error.contains(kestra_refusal(op)), "{template}: {error}");
        }
        for (template, ctx, want) in [
            ("{{ n < 1 }}", json!({"n": null}), "false"),
            ("{{ 1 < n }}", json!({"n": null}), "false"),
            ("{{ n >= m }}", json!({"n": null, "m": null}), "false"),
            ("{{ n <= 1 }}", json!({"n": null}), "false"),
            ("{{ n > 1 }}", json!({"n": null}), "false"),
            ("{{ n < 'a' }}", json!({"n": null}), "false"),
            ("{{ 1.5 > 1 }}", json!({}), "true"),
            ("{{ 'a' < 'b' }}", json!({}), "true"),
            ("{{ 1 == 1 == true }}", json!({}), "true"),
            ("{{ 1 == 2 == false }}", json!({}), "true"),
            ("{{ 1 != 2 != false }}", json!({}), "true"),
            ("{{ 1 < 2 == true }}", json!({}), "true"),
            ("{{ (1e400 - 1e400) < 1 }}", json!({}), "false"),
            (
                "{{ (1e400 - 1e400) >= (1e400 - 1e400) }}",
                json!({}),
                "false",
            ),
        ] {
            let cx = RenderContext::from_json_object(&ctx).unwrap();
            assert_eq!(render_str(template, &cx).unwrap(), want, "{template}");
        }
    }

    /// `String.compareTo` orders UTF-16 code units: a supplementary character
    /// (a surrogate pair, `0xD83D…`) sorts BELOW U+FF21, where Rust's `str`
    /// order (code points) puts it above.
    #[test]
    fn strings_compare_by_utf16_code_units_as_java_does() {
        assert_eq!(render("{{ '\u{1F600}' < '\u{FF21}' }}").unwrap(), "true");
        assert_eq!(render("{{ '\u{FF21}' > '\u{1F600}' }}").unwrap(), "true");
        assert_eq!(render("{{ 'B' < 'a' }}").unwrap(), "true");
        assert_eq!(render("{{ 'ab' <= 'ab' }}").unwrap(), "true");
        assert_eq!(render("{{ 'abc' > 'ab' }}").unwrap(), "true");
    }

    /// Precedence, associativity and statement positions: each shape renders
    /// as it did, and each was compiled (so the agreement is not vacuous).
    #[test]
    fn precedence_and_statement_positions_are_minijinjas() {
        let ctx = json!({
            "a": 2, "b": 3, "c": 3, "l": [1, 2, 3], "s": "ab", "t": true, "m": {"k": 1},
        });
        let shapes = [
            "{{ not a == b }}",
            "{{ not not a == b }}",
            "{{ a ~ b == c }}",
            "{{ a ~ 'x' == '2x' }}",
            "{{ a + 1 > b * 2 }}",
            "{{ a - 1 >= b // 2 }}",
            "{{ a ** 2 >= b }}",
            "{{ -a < b }}",
            "{{ l | length > 2 }}",
            "{{ a is defined == true }}",
            "{{ a is not defined == false }}",
            "{{ (a > b) == (b > c) }}",
            "{{ ((a > b)) }}",
            "{{ a > b and b > c or not c in l }}",
            "{{ a > b or b == c and c in l }}",
            "{{ a if a > b else b }}",
            "{{ 'y' if a > b else 'n' if a == b else 'z' }}",
            "{{ [a > b, b > c] }}",
            "{{ {'k': a > b, 'j': a < b} }}",
            "{{ 'x' | default(a > b) }}",
            "{{ l[a > b] }}",
            "{{ l[0:a > 1] }}",
            "{{ m.k == 1 }}",
            "{{ m['k'] != 1 }}",
            "{{ a not in l }}",
            "{{ not a not in l }}",
            "{{ 'a' in s }}",
            "{{ s in 'xaby' }}",
            "{{ (a > b) is defined }}",
            "{% if a > b %}x{% elif a == b %}y{% else %}z{% endif %}",
            "{% if not a > b %}x{% endif %}",
            "{% for x in l if x > 1 %}{{ x }}{% endfor %}",
            "{% for x in l if x > 1 and x != 3 %}{{ x }}{% endfor %}",
            "{% for x in l %}{{ x > 1 }}{% endfor %}",
            "{% for x in [a > b, a < b] %}{{ x }}{% endfor %}",
            "{% set u = a > b %}{{ u }}",
            "{% set u = a > b, b < c %}{{ u }}",
            "{% set u %}{{ a > b }}{% endset %}{{ u }}",
            "{% with u = a > b, w = a < b %}{{ u }}{{ w }}{% endwith %}",
            "{% filter upper %}{{ a > b }}{% endfilter %}",
            "{{- a > b -}}",
            "{%- if a > b -%}x{%- endif -%}",
            // The first pass's own output composes.
            "{{ a ?? b > c }}",
            "{{ a > b ? 'y' : 'n' }}",
            "{{ nothere ?? 1 > 0 }}",
        ];
        for template in shapes {
            let (old, new, compiled) = old_and_new(template, &ctx);
            assert!(
                compiled.contains("kestra_"),
                "{template} was not compiled: {compiled}"
            );
            assert!(old.is_ok(), "{template}: {old:?}");
            assert_eq!(new, old, "{template} (compiled: {compiled})");
        }
        // The shapes whose answer D1 changed: a chain is left-associative,
        // and a relational operator over a string and a number, or a boolean,
        // fails as Kestra fails.
        let cx = RenderContext::from_json_object(&ctx).unwrap();
        for (template, want) in [
            ("{{ a > b > c }}", Err(">")),
            ("{{ a < b > c }}", Err(">")),
            ("{{ a > b is defined }}", Err(">")),
            ("{{ l | length > 2 | string }}", Err(">")),
            ("{{ a < b == true }}", Ok("true")),
            ("{{ a isIn l == true }}", Ok("true")),
            ("{{ a < b == c in [false] }}", Ok("true")),
        ] {
            match (render_str(template, &cx), want) {
                (Ok(got), Ok(want)) => assert_eq!(got, want, "{template}"),
                (Err(error), Err(op)) => {
                    let error = error.to_string();
                    assert!(error.contains(kestra_refusal(op)), "{template}: {error}");
                }
                (got, want) => panic!("{template}: {got:?}, want {want:?}"),
            }
        }
    }

    /// The shape the pass writes: the call name right after the preceding
    /// token, the operator replaced in place, every other byte kept.
    #[test]
    fn the_compiled_text_is_the_source_with_calls_around_each_comparison() {
        for (source, want) in [
            ("{{ a > b }}", "{{ kestra_gt( a , b) }}"),
            ("{{a==b}}", "{{ kestra_eq(a,b)}}"),
            // The gap between `not` and `in` is kept too.
            ("{{ a not in b }}", "{{ kestra_not_in( a ,  b) }}"),
            // A chain, left-associatively (D1).
            (
                "{{ a < b <= c }}",
                "{{ kestra_le( kestra_lt( a , b ), c) }}",
            ),
            (
                "{{ a in b not in c }}",
                "{{ kestra_not_in( kestra_in( a , b ),  c) }}",
            ),
            (
                "{{ a == b != c > d }}",
                "{{ kestra_gt( kestra_ne( kestra_eq( a , b ), c ), d) }}",
            ),
            ("{% if not a > b %}", "{% if not kestra_gt( a , b) %}"),
            (
                "{{ (a > b) == c }}",
                "{{ kestra_eq( ( kestra_gt(a , b)) , c) }}",
            ),
            ("x > y {{ 1 }} a == b", "x > y {{ 1 }} a == b"),
            ("{# a > b #}{{ 1 }}", "{# a > b #}{{ 1 }}"),
        ] {
            assert_eq!(compiled(source), want, "{source}");
        }
    }

    /// A string literal is one token: its `>` and `==` are text.
    #[test]
    fn a_comparison_inside_a_string_literal_is_left_alone() {
        for source in [
            "{{ 'a > b' }}",
            "{{ \"x == y\" ~ 'z' }}",
            "{{ 'a }} > b' }}",
            "{{ 'it''s' }}",
            "{{ '\\' > ' }}",
        ] {
            assert_eq!(compiled(source), source, "{source}");
        }
        assert_eq!(render("{{ 'a > b' ~ 'c' }}").unwrap(), "a > bc");
        assert_eq!(render("{{ ('a > b' ~ 'c') == 'a > bc' }}").unwrap(), "true");
        assert_eq!(
            compiled("{{ 'a > b' == 'c' }}"),
            "{{ kestra_eq( 'a > b' , 'c') }}"
        );
    }

    /// What minijinja refuses to parse is left for minijinja to refuse, in
    /// its own words.
    #[test]
    fn a_region_the_pass_cannot_parse_is_left_untouched() {
        for source in [
            "{{ a > }}",
            "{{ > b }}",
            "{{ a not b }}",
            "{{ a > b c }}",
            "{% if a > b if c %}x{% endif %}",
            "{% do a > b %}",
            "{% elseif a > b %}",
            "{{ a > b",
            "{{ (a > b }}",
            "{{ a > b) }}",
            "{{ 'unterminated > }}",
        ] {
            assert_eq!(compiled(source), source, "{source}");
        }
        // And the refusal is word for word the one before.
        let ctx = json!({});
        for source in ["{{ a > }}", "{{ a not b }}", "{% do a > b %}"] {
            let (old, new, _) = old_and_new(source, &ctx);
            assert!(old.is_err(), "{source}");
            assert_eq!(new, old, "{source}");
        }
    }

    /// An undefined operand raises the VM's own error, at the VM's own line,
    /// and a silent undefined (a missing `else`) does not raise at all.
    #[test]
    fn an_undefined_operand_raises_the_same_error_on_the_same_line() {
        let ctx = json!({"a": 1, "l": [1]});
        for template in [
            "{{ nothere > 1 }}",
            "{{ 1 == nothere }}",
            "{{ nothere in l }}",
            "{{ 1 in nothere }}",
            "{{ 1 not in nothere }}",
            "{{\n  a > nothere\n}}",
            "line one\n{{ a\n  >\n  nothere }}\nafter {{ nothere2 }}",
            "{% if\n  nothere > 1 %}x{% endif %}",
            "{% for x in l if\n x > nothere %}{{ x }}{% endfor %}",
            "{{ (1 if false) == (2 if false) }}",
            "{{ 1 in (1 if false) }}",
        ] {
            let (old, new, compiled) = old_and_new(template, &ctx);
            assert_eq!(new, old, "{template} (compiled: {compiled})");
        }
        // minijinja names the line of the operand it evaluated last, for the
        // operator and for the call alike.
        let (old, new, _) = old_and_new("{{\n  a\n  >\n  nothere\n}}", &ctx);
        assert_eq!(old.unwrap_err(), "undefined value (in <string>:4)");
        assert_eq!(new.unwrap_err(), "undefined value (in <string>:4)");
        // A chain evaluates every operand before its outer comparison, as
        // Pebble's left-associative tree does, so a strictly undefined third
        // operand raises even after a `false` (D1; Kestra's
        // `chain-lt-missing-operand` raises too).
        for template in ["{{ 1 > 2 > nothere }}", "{{ 1 < 2 > nothere }}"] {
            let (_, new, _) = old_and_new(template, &ctx);
            assert_eq!(
                new.unwrap_err(),
                "undefined value (in <string>:1)",
                "{template}"
            );
        }
        // A silent undefined (a missing `else`) is egret's null: `false`.
        let (_, new, _) = old_and_new("{{ (1 if false) > 1 }}", &ctx);
        assert_eq!(new.unwrap(), "false");
    }

    /// Parenthesised comparisons nested to minijinja's own limit compile and
    /// render; past it, the region is left for minijinja to refuse.
    #[test]
    fn deep_nesting_is_bounded() {
        let deep = |n: usize| {
            format!(
                "{{{{ {}1 > 0{} }}}}",
                "(1 > 0) == (".repeat(n),
                ")".repeat(n)
            )
        };
        let source = deep(40);
        let deep_compiled = compiled(&source);
        assert_eq!(deep_compiled.matches("kestra_").count(), 81);
        assert_eq!(render(&source).unwrap(), "true");
        let past = deep(400);
        assert_eq!(compiled(&past), past);
        // On a small stack, the pass alone.
        let handle = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(move || compiled(&deep(MAX_DEPTH_PROBE)))
            .unwrap();
        handle
            .join()
            .expect("the pass does not overflow a 256 KiB stack");
    }

    const MAX_DEPTH_PROBE: usize = 155;

    /// N1 (W9c Task 4 cleanup): a raw block is skipped where it stands,
    /// whatever its whitespace control, and the comparisons around it are
    /// still compiled. egret's raw protection does not know `{%+ raw %}`, so
    /// this pass is what sees it.
    #[test]
    fn a_raw_block_is_skipped_where_it_stands() {
        // The review's example: before, the `+` form turned the pass off for
        // the whole template and this rendered `xtrue|false`.
        assert_eq!(
            render(
                "{% set big = '1.5' | number(type='BIGDECIMAL') %}\
                 {%+ raw %}x{% endraw %}{{ big > 100 }}|{{ big == 1.50 }}"
            )
            .unwrap(),
            "xfalse|true"
        );
        for open in [
            "{% raw %}",
            "{%+ raw %}",
            "{%+ raw -%}",
            "{%- raw +%}",
            "{%+raw%}",
        ] {
            for close in [
                "{% endraw %}",
                "{%+ endraw %}",
                "{%- endraw -%}",
                "{%endraw+%}",
            ] {
                let source = format!("{open}{{{{ a > b }}}}{close}{{{{ a > b }}}}");
                assert_eq!(
                    compiled(&source),
                    format!("{open}{{{{ a > b }}}}{close}{{{{ kestra_gt( a , b) }}}}"),
                    "{source}"
                );
            }
        }
        // The body is data up to the FIRST endraw tag, as minijinja reads it.
        assert_eq!(
            compiled("{%+ raw %}{% if a > b %}{%+ endraw %}{% if a > b %}{% endif %}"),
            "{%+ raw %}{% if a > b %}{%+ endraw %}{% if kestra_gt( a , b) %}{% endif %}"
        );
        // No endraw, or a `raw` that is not a raw tag: minijinja refuses, so
        // the template is left whole for it to refuse, in its own words.
        for source in [
            "{%+ raw %}{{ a > b }}",
            "{{ a > b }}{%+ raw %}",
            "{% raw x %}{{ a > b }}{% endraw %}",
        ] {
            assert_eq!(compiled(source), source, "{source}");
            let (old, new, _) = old_and_new(source, &json!({"a": 1, "b": 2}));
            assert!(old.is_err(), "{source}: {old:?}");
            assert_eq!(new, old, "{source}");
        }
    }

    /// N2 (W9c Task 4 cleanup): a template variable named like one of the
    /// rewriters' functions would shadow it, so a `kestra_` assignment
    /// target is refused at compile time; nothing else named `kestra…` is.
    #[test]
    fn a_kestra_prefixed_assignment_target_is_refused() {
        for (template, name) in [
            ("{% set kestra_gt = 1 %}{{ 1 > 2 }}", "kestra_gt"),
            ("{%- set kestra_lt = 1 -%}", "kestra_lt"),
            ("{% set x, kestra_ne = 1, 2 %}", "kestra_ne"),
            ("{% set kestra_is_in %}x{% endset %}", "kestra_is_in"),
            ("{% set kestra_eq | upper %}x{% endset %}", "kestra_eq"),
            (
                "{% for kestra_eq in [1] %}{{ 1 == 1 }}{% endfor %}",
                "kestra_eq",
            ),
            (
                "{% for k, kestra_le in {'a': 1} | items %}{% endfor %}",
                "kestra_le",
            ),
            (
                "{% with kestra_null_coalesce = 1 %}{% endwith %}",
                "kestra_null_coalesce",
            ),
            ("{% with x = 1, kestra_in = 2 %}{% endwith %}", "kestra_in"),
            ("{% with (a, kestra_x) = (1, 2) %}{% endwith %}", "kestra_x"),
            // In a template with no comparison at all, too.
            (
                "{% set kestra_undefined_coalesce = 1 %}",
                "kestra_undefined_coalesce",
            ),
        ] {
            let error = render(template).unwrap_err();
            assert_eq!(
                error,
                format!(
                    "`{name}` cannot be assigned: names starting with `kestra_` are reserved \
                     for egret's own functions (a `set`, `for` or `with` target)"
                ),
                "{template}"
            );
        }
        for (template, want) in [
            ("{% set kestra = 1 %}{{ kestra > 0 }}", "true"),
            ("{% set my_kestra_gt = 1 %}{{ my_kestra_gt > 0 }}", "true"),
            (
                "{% for kestrax in [1] %}{{ kestrax == 1 }}{% endfor %}",
                "true",
            ),
            (
                "{% set m = {'kestra_gt': 1} %}{{ m.kestra_gt > 0 }}",
                "true",
            ),
            ("{{ 'kestra_gt' }}{# set kestra_gt = 1 #}", "kestra_gt"),
            (
                "{% raw %}{% set kestra_gt = 1 %}{% endraw %}",
                "{% set kestra_gt = 1 %}",
            ),
        ] {
            assert_eq!(render(template).unwrap(), want, "{template}");
        }
        // What minijinja refuses to parse keeps minijinja's own refusal.
        let (old, new, _) = old_and_new("{% set kestra_gt = %}", &json!({}));
        assert!(old.is_err());
        assert_eq!(new, old);
    }

    /// N4 (W9c Task 4 cleanup): the rows the differential could not hold.
    /// A `BigNumber` against a non-number takes minijinja's own answer
    /// (`exact_of` gives `None`), so old and new agree; against a
    /// non-finite float it answers as a finite float of the same sign would.
    #[test]
    fn a_big_number_against_a_non_number_or_a_non_finite_float() {
        let ctx = context();
        let bigs = [
            "('1.5' | number(type='BIGDECIMAL'))",
            "('-2.50' | number(type='BIGDECIMAL'))",
            "('123456789012345678901234567890123456789012' | number(type='BIGINTEGER'))",
        ];
        let non_numbers = [
            "'a'",
            "''",
            "'1.5'",
            "none",
            "true",
            "false",
            "[]",
            "['a']",
            "{}",
            "{'a': 1}",
            "nothere",
            "(1 if false)",
            "cs",
            "cn",
            "cm",
        ];
        let (mut checked, mut errors) = (0usize, 0usize);
        for big in bigs {
            for other in non_numbers {
                for op in OPS {
                    for template in [
                        format!("{{{{ {big} {op} {other} }}}}"),
                        format!("{{{{ {other} {op} {big} }}}}"),
                    ] {
                        let (old, new, compiled) = old_and_new(&template, &ctx);
                        assert!(compiled.contains("kestra_"), "{template}");
                        checked += 1;
                        if !RELATIONAL_OPS.contains(op) {
                            assert_eq!(new, old, "{template} (compiled: {compiled})");
                            errors += usize::from(old.is_err());
                            continue;
                        }
                        // D1: a null is `false`, an undefined raises as
                        // before, anything else is Kestra's refusal.
                        match class(other) {
                            Class::Null => assert_eq!(new.as_deref(), Ok("false"), "{template}"),
                            Class::Undefined => assert_eq!(new, old, "{template}"),
                            _ => {
                                let error = new.expect_err(&template);
                                assert!(error.contains(kestra_refusal(op)), "{template}: {error}");
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(checked, bigs.len() * non_numbers.len() * OPS.len() * 2);
        assert!(
            errors > 0 && errors < checked,
            "{errors} of {checked} refused"
        );

        // Against inf, -inf and NaN of either sign: the same answer as a
        // finite float on the same side of zero.
        let stand_ins = [(bigs[0], "1.5"), (bigs[1], "-2.5"), (bigs[2], "1e300")];
        let non_finite = ["1e400", "-1e400", "(1e400 - 1e400)", "-(1e400 - 1e400)"];
        for (big, float) in stand_ins {
            for x in non_finite {
                for op in ["==", "!=", "<", "<=", ">", ">="] {
                    for (left, right) in [
                        (
                            format!("{{{{ {big} {op} {x} }}}}"),
                            format!("{{{{ {float} {op} {x} }}}}"),
                        ),
                        (
                            format!("{{{{ {x} {op} {big} }}}}"),
                            format!("{{{{ {x} {op} {float} }}}}"),
                        ),
                    ] {
                        assert_eq!(render(&left), render(&right), "{left} vs {right}");
                    }
                }
            }
        }
        for (template, want) in [
            ("{{ ('1.5' | number(type='BIGDECIMAL')) < 1e400 }}", "true"),
            ("{{ ('1.5' | number(type='BIGDECIMAL')) > -1e400 }}", "true"),
            (
                "{{ ('1.5' | number(type='BIGDECIMAL')) == (1e400 - 1e400) }}",
                "false",
            ),
            (
                "{{ ('1.5' | number(type='BIGDECIMAL')) != (1e400 - 1e400) }}",
                "true",
            ),
        ] {
            assert_eq!(render(template).unwrap(), want, "{template}");
        }
    }

    #[test]
    fn exact_values_order_and_equal_as_numbers() {
        let p = |s: &str| Exact::parse_plain(s).unwrap();
        assert_eq!(p("1.5").cmp(&p("1.50")), Ordering::Equal);
        assert_eq!(p("0.00").cmp(&p("0")), Ordering::Equal);
        assert_eq!(p("-0.0").cmp(&p("0")), Ordering::Equal);
        assert_eq!(p("10.5").cmp(&p("9.5")), Ordering::Greater);
        assert_eq!(p("-1.5").cmp(&p("-2.5")), Ordering::Greater);
        assert_eq!(p("-2.5").cmp(&p("-1.5")), Ordering::Less);
        assert_eq!(p("0.001").cmp(&p("0.01")), Ordering::Less);
        assert_eq!(p("100").cmp(&p("99.999")), Ordering::Greater);
        assert_eq!(p("1.25").cmp(&p("1.2")), Ordering::Greater);
        let big = "9".repeat(60);
        assert_eq!(
            p(&big).cmp(&p(&format!("1{}", "0".repeat(60)))),
            Ordering::Less
        );
        // A float is its exact binary value.
        assert_eq!(Exact::of_f64(1.5).cmp(&p("1.5")), Ordering::Equal);
        assert_eq!(Exact::of_f64(0.1).cmp(&p("0.1")), Ordering::Greater);
        assert_eq!(
            Exact::of_f64(0.1).cmp(&p(
                "0.1000000000000000055511151231257827021181583404541015625"
            )),
            Ordering::Equal
        );
        assert_eq!(Exact::of_f64(-0.0).cmp(&p("0")), Ordering::Equal);
        assert_eq!(
            Exact::of_f64(f64::INFINITY).cmp(&p(&big)),
            Ordering::Greater
        );
        assert_eq!(
            Exact::of_f64(f64::NEG_INFINITY).cmp(&p("-1")),
            Ordering::Less
        );
        assert_eq!(Exact::of_f64(5e-324).cmp(&p("0")), Ordering::Greater);
        assert_eq!(Exact::of_f64(f64::MAX).cmp(&p(&big)), Ordering::Greater);
        assert_eq!(Exact::of_f64(f64::NAN).cmp(&p(&big)), Ordering::Greater);
        assert_eq!(Exact::of_f64(-f64::NAN).cmp(&p("-1")), Ordering::Less);
        assert!(Exact::parse_plain("1e5").is_none());
        assert!(Exact::parse_plain("").is_none());
    }

    /// `number(type='BIGDECIMAL')` is the same carrier, and the same fix.
    #[test]
    fn number_bigdecimal_compares_exactly() {
        for (template, want) in [
            ("{{ '1' | number(type='BIGDECIMAL') > 1 }}", "false"),
            ("{{ '1.5' | number(type='BIGDECIMAL') == 1.5 }}", "true"),
            ("{{ '1.50' | number(type='BIGDECIMAL') == 1.5 }}", "true"),
            ("{{ '1.5' | number(type='BIGDECIMAL') < 100 }}", "true"),
            (
                "{{ '-1.5' | number(type='BIGDECIMAL') > '-2.5' | number(type='BIGDECIMAL') }}",
                "true",
            ),
            ("{{ '1.5' | number(type='BIGDECIMAL') in [1.5] }}", "true"),
            ("{{ '0.1' | number(type='BIGDECIMAL') == 0.1 }}", "false"),
            ("{{ '0.5' | number(type='BIGDECIMAL') == 0.5 }}", "true"),
            ("{{ '1.5' | number(type='BIGDECIMAL') == '1.5' }}", "false"),
        ] {
            assert_eq!(render(template).unwrap(), want, "{template}");
        }
        // Arithmetic is Kestra's since package #5a Task A4 (spec D3).
        assert_eq!(
            render("{{ '1.5' | number(type='BIGDECIMAL') + 1 }}").unwrap(),
            "2.5"
        );
    }

    /// `sort` orders `BigDecimal`s by value; it refused them before.
    #[test]
    fn sort_orders_big_decimals_by_value() {
        let n = |v: &str| format!("'{v}' | number(type='BIGDECIMAL')");
        let list = ["10.5", "9.5", "-1", "1.50", "-2.5", "1.5"]
            .map(n)
            .join(", ");
        assert_eq!(
            render(&format!("{{{{ [{list}] | sort }}}}")).unwrap(),
            "[-2.5,-1,1.50,1.5,9.5,10.5]"
        );
        let big = ["9", "10", "123456789012345678901234567890123456789012"]
            .map(|v| format!("'{v}' | number(type='BIGINTEGER')"))
            .join(", ");
        assert_eq!(
            render(&format!("{{{{ [{big}] | sort }}}}")).unwrap(),
            "[9,10,123456789012345678901234567890123456789012]"
        );
    }
}
