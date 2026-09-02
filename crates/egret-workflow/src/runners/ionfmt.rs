// kestra: core/src/main/java/io/kestra/core/runners/pebble/filters/ToIonFilter.java
// kestra: core/src/main/java/io/kestra/core/serializers/JacksonMapper.java (ofIon)

//! Amazon Ion **text** as `JacksonMapper.ofIon()` writes it — the one thing
//! `toIon` does.
//!
//! # Why this is hand-written and not `ion-rs`
//!
//! W2 Task 6's brief names `ion-rs` 1.0. It was tried first and it cannot
//! produce this output. Measured on `ion-rs` 1.0.1, the same value through
//! **all three** public `TextFormat`s (`Compact`, `Lines`, `Pretty`), via
//! `Element::encode_as(WriteConfig::<v1_0::Text>::new(...))`:
//!
//! | | Kestra (`golden.json`) | `ion-rs` 1.0.1, every format |
//! |---|---|---|
//! | `{a:1}` | `{a:1}` | `{a: 1}` — a space after every `:` and `,` |
//! | a list | `[1,2]` | `[1, 2, ]` — a trailing comma, structs too |
//! | `1e100` | `1.0e100` | `1e100` |
//! | `"a\x01b"` | `"a\x01b"` | the raw control byte, unescaped |
//!
//! **An earlier version of this note said the writer types are `pub(crate)`.
//! That was wrong**: `WriteConfig`, `TextFormat` and `v1_0::Text` are all
//! public on default features, and `Writer`/`TextWriter_1_0` are behind the
//! opt-in `experimental-reader-writer` feature — which the macro spells as
//! `pub(crate)` when the feature is off, and which is what the compiler error
//! that produced the claim actually meant. The claim was a "does not expose"
//! sentence written from one error message rather than from the crate, which
//! is the mistake `oracle/README.md`'s C9 rule exists to prevent, aimed at a
//! Rust crate instead of a Java one.
//!
//! **The decision is unchanged, and now rests on the measurement above rather
//! than on the visibility**: no public format produces Kestra's layout, and
//! enabling the experimental feature does not change the four rows.
//! Post-processing that into Kestra's form would be a second serialiser
//! wrapped around the first. This module is the first one instead, and every
//! rule in it is read off the harness (see `oracle/README.md`, "What W2
//! Task 6's sweeps measured").
//!
//! # `toIon` keeps null map values and `toJson` does not
//!
//! `JacksonMapper`'s shared `configure()` sets
//! `setDefaultPropertyInclusion(NON_NULL)`, and `createIonObjectMapper`
//! **overrides it back to `ALWAYS`** — confirmed by `javap -p -c` on
//! `JacksonMapper.class`. So `{"a":null,"b":1}` is `{"b":1}` through `toJson`
//! and `{a:null,b:1}` through `toIon`, out of the same batch of filters.

use minijinja::value::{Value as MjValue, ValueKind};

use super::env::write_java_double;

/// Writes `value` as Ion text, compact, into `out`.
pub(super) fn write_ion(out: &mut String, value: &MjValue) {
    match value.kind() {
        ValueKind::Undefined | ValueKind::None => out.push_str("null"),
        ValueKind::Bool => out.push_str(if value.is_true() { "true" } else { "false" }),
        ValueKind::Number => {
            if value.is_integer() {
                out.push_str(&value.to_string());
            } else {
                match f64::try_from(value.clone()) {
                    Ok(f) => write_ion_float(out, f),
                    Err(_) => out.push_str(&value.to_string()),
                }
            }
        }
        ValueKind::Seq | ValueKind::Iterable => {
            out.push('[');
            let mut first = true;
            if let Ok(items) = value.try_iter() {
                for item in items {
                    if !first {
                        out.push(',');
                    }
                    first = false;
                    write_ion(out, &item);
                }
            }
            out.push(']');
        }
        ValueKind::Map => {
            out.push('{');
            let mut first = true;
            if let Ok(keys) = value.try_iter() {
                for key in keys {
                    let Ok(item) = value.get_item(&key) else {
                        continue;
                    };
                    if !first {
                        out.push(',');
                    }
                    first = false;
                    write_symbol(out, &key.to_string());
                    out.push(':');
                    write_ion(out, &item);
                }
            }
            out.push('}');
        }
        // A `BigDecimal`/`BigInteger` is a number here too, and Ion spells a
        // decimal's exponent with `d` rather than `E`:
        // `'1E3' | number(type='BIGDECIMAL') | toIon` is `1d3`, measured.
        // Quoting it as a string -- which an earlier cut did, justifying
        // itself by matching `write_json`'s catch-all, which had the same
        // bug -- is not a near miss: `1d3` is a number and `"1000"` is text.
        _ if super::filters::big_number_text(value).is_some() => {
            let text = super::filters::big_number_text(value).expect("checked");
            match text.split_once('E') {
                Some((mantissa, exponent)) => {
                    out.push_str(mantissa);
                    out.push('d');
                    out.push_str(exponent.strip_prefix('+').unwrap_or(exponent));
                }
                None => out.push_str(&text),
            }
        }
        // Strings.
        _ => write_ion_string(out, &value.to_string()),
    }
}

/// Ion's float text, which is **not** `Double.toString` even though it starts
/// from it.
///
/// Measured over 27 values through the harness. `Double.toString` output is
/// transformed by exactly two rules:
///
/// - it already has an exponent (`1.0E100`) -> lowercase the `E`, keep the
///   mantissa: `1.0e100`;
/// - it has none (`1.0`, `2.5`, `9999999.0`) -> drop a trailing `.0` and
///   append `e0`: `1e0`, `2.5e0`, `9999999e0`.
///
/// So `1.0` and `1.0e100` disagree about whether the `.0` survives, which no
/// single rule about "trailing zeros" predicts. `-0.0` is `-0e0`.
fn write_ion_float(out: &mut String, x: f64) {
    // `Double.toString`'s three special values. Ion text spells them
    // differently; Jackson's `IonGenerator.writeNumber(double)` hands them to
    // `IonWriter.writeFloat`, which writes these. No corpus entry reaches
    // them (a JSON context cannot carry one) and none has a golden.
    if x.is_nan() {
        out.push_str("nan");
        return;
    }
    if x.is_infinite() {
        out.push_str(if x > 0.0 { "+inf" } else { "-inf" });
        return;
    }

    let mut java = String::new();
    write_java_double(&mut java, x);
    match java.split_once('E') {
        Some((mantissa, exponent)) => {
            out.push_str(mantissa);
            out.push('e');
            out.push_str(exponent);
        }
        None => {
            out.push_str(java.strip_suffix(".0").unwrap_or(&java));
            out.push_str("e0");
        }
    }
}

/// A `"`-delimited Ion string.
fn write_ion_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            _ => write_ion_char(out, c),
        }
    }
    out.push('"');
}

/// A struct field name: a bare symbol where Ion allows one, `'`-quoted
/// otherwise.
///
/// Bare requires an identifier (`[A-Za-z$_][A-Za-z0-9$_]*`) that is not one of
/// Ion's keywords. Measured: `$a` and `_a` are bare, `true`/`false`/`null` are
/// quoted, and `True`/`TRUE`/`Null`/`yes`/`on`/`y` are **bare** — Ion's
/// keyword set is case-sensitive and much smaller than YAML's, which is the
/// opposite of what the `yaml` filter in the same batch does with the same
/// keys.
fn write_symbol(out: &mut String, s: &str) {
    if is_ion_identifier(s) {
        out.push_str(s);
        return;
    }
    out.push('\'');
    for c in s.chars() {
        match c {
            '\'' => out.push_str("\\'"),
            _ => write_ion_char(out, c),
        }
    }
    out.push('\'');
}

/// One character inside either quoting form. The quote character itself is
/// handled by the caller, because the two forms escape different ones: a `"`
/// is literal inside `'…'` and a `'` is literal inside `"…"`, both measured.
fn write_ion_char(out: &mut String, c: char) {
    match c {
        '\\' => out.push_str("\\\\"),
        '\n' => out.push_str("\\n"),
        '\r' => out.push_str("\\r"),
        '\t' => out.push_str("\\t"),
        '\u{0b}' => out.push_str("\\v"),
        '\u{0c}' => out.push_str("\\f"),
        '\u{08}' => out.push_str("\\b"),
        '\u{07}' => out.push_str("\\a"),
        '\0' => out.push_str("\\0"),
        // C0, DEL and C1. `\u{85}` is escaped and `\u{a0}` is not, which is
        // exactly this boundary; both were measured.
        c if (c as u32) < 0x20 || (0x7f..=0x9f).contains(&(c as u32)) => {
            out.push_str(&format!("\\x{:02x}", c as u32));
        }
        c => out.push(c),
    }
}

/// `[A-Za-z$_][A-Za-z0-9$_]*`, minus Ion's keywords.
fn is_ion_identifier(s: &str) -> bool {
    // `$123` is a symbol ID in Ion text, not an identifier, so it has to be
    // quoted to mean the name.
    if s.len() > 1 && s.starts_with('$') && s[1..].bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    // Ion's typed nulls (`null.int`, `null.list`, …) need no clause of their
    // own: a `.` is not an identifier character, so the loop below rejects
    // them anyway. An earlier cut carried `s.starts_with("null.")` here and
    // it was unreachable.
    if matches!(s, "null" | "true" | "false" | "nan") {
        return false;
    }
    let mut chars = s.chars();
    match chars.next() {
        None => false,
        Some(c) if c.is_ascii_alphabetic() || c == '$' || c == '_' => {
            chars.all(|c| c.is_ascii_alphanumeric() || c == '$' || c == '_')
        }
        Some(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ion(value: MjValue) -> String {
        let mut out = String::new();
        write_ion(&mut out, &value);
        out
    }

    /// The float rule, on the values that separate it from `Double.toString`
    /// and from a naive "always keep the mantissa" reading. Every expectation
    /// is a golden this task measured through the Java harness.
    #[test]
    fn ion_floats_drop_the_trailing_zero_only_when_there_is_no_exponent() {
        assert_eq!(ion(MjValue::from(1.0_f64)), "1e0");
        assert_eq!(ion(MjValue::from(2.5_f64)), "2.5e0");
        assert_eq!(ion(MjValue::from(9_999_999.0_f64)), "9999999e0");
        assert_eq!(ion(MjValue::from(1e100_f64)), "1.0e100");
        assert_eq!(ion(MjValue::from(1e-7_f64)), "1.0e-7");
        assert_eq!(ion(MjValue::from(1e7_f64)), "1.0e7");
        assert_eq!(ion(MjValue::from(-0.0_f64)), "-0e0");
        assert_eq!(ion(MjValue::from(123_456_789.125_f64)), "1.23456789125e8");
    }

    #[test]
    fn ion_quotes_a_field_name_only_when_it_is_not_an_identifier() {
        assert!(is_ion_identifier("a"));
        assert!(is_ion_identifier("$a"));
        assert!(is_ion_identifier("_a"));
        assert!(is_ion_identifier("True"));
        assert!(is_ion_identifier("yes"));
        assert!(!is_ion_identifier("true"));
        assert!(!is_ion_identifier("null"));
        assert!(!is_ion_identifier("nan"));
        assert!(!is_ion_identifier("42"));
        assert!(!is_ion_identifier("a b"));
        assert!(!is_ion_identifier(""));
        assert!(!is_ion_identifier("$1"));
    }

    #[test]
    fn ion_escapes_c0_del_and_c1_but_not_nbsp() {
        assert_eq!(ion(MjValue::from("a\u{1}b")), "\"a\\x01b\"");
        assert_eq!(ion(MjValue::from("a\u{7f}b")), "\"a\\x7fb\"");
        assert_eq!(ion(MjValue::from("a\u{85}b")), "\"a\\x85b\"");
        assert_eq!(ion(MjValue::from("a\u{a0}b")), "\"a\u{a0}b\"");
        assert_eq!(ion(MjValue::from("a\u{b}b")), "\"a\\vb\"");
    }

    /// The quote character is escaped in one form and literal in the other,
    /// which is the one rule a shared escaper would get wrong.
    #[test]
    fn ion_escapes_the_delimiter_of_the_form_it_is_writing() {
        assert_eq!(ion(MjValue::from("a'b")), "\"a'b\"");
        assert_eq!(ion(MjValue::from("a\"b")), "\"a\\\"b\"");
        let mut symbol = String::new();
        write_symbol(&mut symbol, "a\"b");
        assert_eq!(symbol, "'a\"b'");
        let mut symbol = String::new();
        write_symbol(&mut symbol, "a'b");
        assert_eq!(symbol, "'a\\'b'");
    }
}
