// kestra: io.pebbletemplates.pebble.extension.core.FormatFilter -> java.util.Formatter

//! `java.util.Formatter`'s `%` grammar, for Pebble's `format` filter.
//!
//! # Why this exists rather than a wrapper around `minijinja`'s
//!
//! W2 Task 8's fix round wrapped `minijinja::filters::format` on the stated
//! premise that the two agreed on the six specifiers minijinja has. **That
//! premise was false, and two of the six were among the counter-examples:**
//!
//! | expression | Java | `minijinja` |
//! |---|---|---|
//! | `'%x' \| format(-255)` | `ffffff01` — two's complement | `-ff` |
//! | `'%.0f' \| format(0.5)` | `1` — HALF_UP | `0` — half-to-even |
//! | `'%.2f' \| format(1e300)` | the shortest repr, padded | 300 digits of the exact value |
//! | `'%.2f' \| format(1.005)` | `1.01` | `1.00` |
//! | `'%s' \| format(true)` | `true` | `True` |
//! | `'%s' \| format({'a':1})` | `{a=1}` | `{"a": 1}` |
//!
//! Every one of those is a **silently wrong value**, which is worse than the
//! errors a narrower port produces. So the grammar is ported and every
//! conversion this module does not implement is refused **by name** — the same
//! call `super::javafmt` makes for `z`, `v` and `B`.
//!
//! # What is implemented, and what is refused
//!
//! Conversions: `s S b B d o x X f e E % n`. Flags: `-`, `+`, ` `, `0`, `,`,
//! `(`, `#`. Width, precision, and the `N$` argument index.
//!
//! Refused, and recorded in `oracle/name-surface.txt`: `c`, `g`, `G`, `a`, `A`,
//! `h`, `H` and the whole `t`/`T` date family. Refusing is the safe direction —
//! an author gets an error naming the conversion, never a different number.
//!
//! # The three rules that are not obvious, all measured through the harness
//!
//! 1. **A `null` argument is the four characters `null`, through the STRING
//!    path, for every conversion except `b`/`B`.** So `'%.0f' | format(null)`
//!    is the empty string — precision truncates `"null"` to nothing — and
//!    `'%.2f' | format(null)` is `nu`. `%b` of a null is `false`.
//! 2. **`%x`/`%X`/`%o` of a negative print two's complement at the argument's
//!    OWN width**, and which width that is comes from
//!    [`super::filters::java_class_name`]'s magnitude rule: `-255` is an
//!    `Integer` and prints 32 bits (`ffffff01`), `Long.MIN_VALUE` prints 64
//!    (`8000000000000000`).
//! 3. **`%f` rounds the SHORTEST decimal representation, HALF_UP** — not the
//!    exact binary value. That is why `1.005` (whose exact value is
//!    `1.00499999…`) formats as `1.01`, and why `1e300` formats as a 1 and 300
//!    zeros rather than the 300 digits the double really is. Rust's `{}` is
//!    also shortest-round-trip and positional, so it is the right source for
//!    the digits.

use minijinja::value::{Value as MjValue, ValueKind};
use minijinja::Error as MjError;

use super::env::kestra_to_string;
use super::filters::{is_java_null, java_class_name, pebble_error};

/// One `%[index$][flags][width][.precision]conversion`.
struct Spec {
    index: Option<usize>,
    left: bool,
    plus: bool,
    space: bool,
    zero: bool,
    group: bool,
    parens: bool,
    alt: bool,
    width: Option<usize>,
    precision: Option<usize>,
    conversion: char,
}

/// `String.format(format, args)`.
pub(super) fn format(template: &str, args: &[MjValue]) -> Result<String, MjError> {
    let mut out = String::new();
    let bytes: Vec<char> = template.chars().collect();
    let mut at = 0usize;
    // Java's implicit argument cursor: `%s%s` takes args 0 then 1, and an
    // explicit `N$` does not move it.
    let mut next_arg = 0usize;

    while at < bytes.len() {
        if bytes[at] != '%' {
            out.push(bytes[at]);
            at += 1;
            continue;
        }
        let (spec, after) = parse_spec(&bytes, at)?;
        at = after;
        match spec.conversion {
            '%' => {
                out.push_str(&pad(&spec, "%".to_string(), false));
                continue;
            }
            'n' => {
                out.push('\n');
                continue;
            }
            _ => {}
        }
        let which = match spec.index {
            Some(one_based) => one_based.checked_sub(1).unwrap_or(usize::MAX),
            None => {
                let i = next_arg;
                next_arg += 1;
                i
            }
        };
        let Some(argument) = args.get(which) else {
            return Err(pebble_error(format!(
                "Format specifier '%{}'",
                spec.conversion
            )));
        };
        out.push_str(&render(&spec, argument)?);
    }
    Ok(out)
}

/// Reads one specifier, starting at the `%`.
fn parse_spec(chars: &[char], start: usize) -> Result<(Spec, usize), MjError> {
    let mut at = start + 1;
    let digits = |at: &mut usize| -> Option<usize> {
        let from = *at;
        while *at < chars.len() && chars[*at].is_ascii_digit() {
            *at += 1;
        }
        (from < *at).then(|| {
            chars[from..*at]
                .iter()
                .collect::<String>()
                .parse()
                .unwrap_or(0)
        })
    };

    // `N$` is an argument index only if a `$` follows; otherwise the digits
    // were the width (or a `0` flag), so this backtracks.
    let mark = at;
    let mut index = None;
    if let Some(n) = digits(&mut at) {
        if at < chars.len() && chars[at] == '$' {
            index = Some(n);
            at += 1;
        } else {
            at = mark;
        }
    }

    let mut spec = Spec {
        index,
        left: false,
        plus: false,
        space: false,
        zero: false,
        group: false,
        parens: false,
        alt: false,
        width: None,
        precision: None,
        conversion: ' ',
    };
    while at < chars.len() {
        match chars[at] {
            '-' => spec.left = true,
            '+' => spec.plus = true,
            ' ' => spec.space = true,
            '0' => spec.zero = true,
            ',' => spec.group = true,
            '(' => spec.parens = true,
            '#' => spec.alt = true,
            _ => break,
        }
        at += 1;
    }
    spec.width = digits(&mut at);
    if at < chars.len() && chars[at] == '.' {
        at += 1;
        spec.precision = Some(digits(&mut at).unwrap_or(0));
    }
    if at >= chars.len() {
        // Java: a trailing `%` is `UnknownFormatConversionException: '%'`.
        return Err(pebble_error("Conversion = '%'"));
    }
    spec.conversion = chars[at];
    Ok((spec, at + 1))
}

/// One argument through one conversion.
fn render(spec: &Spec, argument: &MjValue) -> Result<String, MjError> {
    let null = is_java_null(argument);
    match spec.conversion {
        'b' | 'B' => {
            let text = if null { "false" } else { "true" }.to_string();
            let text = if spec.conversion == 'B' {
                text.to_uppercase()
            } else {
                text
            };
            Ok(pad(spec, truncate(spec, text), false))
        }
        // **Every remaining conversion sends a null through the string path**,
        // so `'%.0f' | format(null)` is the empty string. Measured.
        _ if null => {
            let text = if spec.conversion.is_uppercase() {
                "NULL".to_string()
            } else {
                "null".to_string()
            };
            Ok(pad(spec, truncate(spec, text), false))
        }
        's' => Ok(pad(spec, truncate(spec, kestra_to_string(argument)), false)),
        'S' => Ok(pad(
            spec,
            truncate(spec, kestra_to_string(argument).to_uppercase()),
            false,
        )),
        'd' | 'o' | 'x' | 'X' => integer(spec, argument),
        'f' | 'e' | 'E' => float(spec, argument),
        // A conversion Java HAS and egret does not: named, and refused.
        'c' | 'C' | 'g' | 'G' | 'a' | 'A' | 'h' | 'H' | 't' | 'T' => Err(pebble_error(format!(
            "the 'format' filter's '%{}' conversion is not implemented in egret; \
             's S b B d o x X f e E % n' are",
            spec.conversion
        ))),
        // A conversion character Java does not have either.
        other => Err(pebble_error(format!("Conversion = '{other}'"))),
    }
}

fn integer(spec: &Spec, argument: &MjValue) -> Result<String, MjError> {
    if spec.precision.is_some() {
        return Err(pebble_error(format!("Precision = {:?}", spec.precision)));
    }
    if argument.kind() != ValueKind::Number || !argument.is_integer() {
        return Err(pebble_error(format!(
            "{} != java.lang.Integer",
            java_class_name(argument)
        )));
    }
    let Ok(n) = i64::try_from(argument.clone()) else {
        return Err(pebble_error(
            "the 'format' filter cannot render this integer",
        ));
    };
    let negative = n < 0;
    let body = match spec.conversion {
        'd' => {
            let mut digits = n.unsigned_abs().to_string();
            if spec.group {
                digits = group(&digits);
            }
            digits
        }
        // Two's complement at the argument's own width, which is the
        // Integer/Long split.
        radix => {
            let wide = java_class_name(argument) != "java.lang.Integer";
            let raw = if wide {
                format!("{:x}", n as u64)
            } else {
                format!("{:x}", n as i32 as u32)
            };
            let raw = if radix == 'o' {
                if wide {
                    format!("{:o}", n as u64)
                } else {
                    format!("{:o}", n as i32 as u32)
                }
            } else {
                raw
            };
            let raw = if radix == 'X' {
                raw.to_uppercase()
            } else {
                raw
            };
            let prefix = match (spec.alt, radix) {
                (true, 'x') => "0x",
                (true, 'X') => "0X",
                (true, 'o') => "0",
                _ => "",
            };
            // A radix conversion has no sign: the sign IS the bit pattern.
            return Ok(pad(spec, format!("{prefix}{raw}"), true));
        }
    };
    let signed = if negative {
        if spec.parens {
            format!("({body})")
        } else {
            format!("-{body}")
        }
    } else if spec.plus {
        format!("+{body}")
    } else if spec.space {
        format!(" {body}")
    } else {
        body
    };
    Ok(pad(spec, signed, true))
}

fn float(spec: &Spec, argument: &MjValue) -> Result<String, MjError> {
    // **An INTEGER is an `IllegalFormatConversionException`, not a widening.**
    // `'%f' | format(42)` is `f != java.lang.Integer` on Kestra, where a port
    // that took any number would answer `42.000000`. Measured across the whole
    // `f`/`e`/`E` row.
    if java_class_name(argument) != "java.lang.Double" {
        return Err(pebble_error(format!(
            "{} != java.lang.Double",
            java_class_name(argument)
        )));
    }
    let Ok(x) = f64::try_from(argument.clone()) else {
        return Err(pebble_error(
            "the 'format' filter cannot render this number",
        ));
    };
    if x.is_nan() || x.is_infinite() {
        let text = if x.is_nan() {
            "NaN".to_string()
        } else if x > 0.0 {
            "Infinity".to_string()
        } else {
            "-Infinity".to_string()
        };
        return Ok(pad(spec, text, false));
    }
    let precision = spec.precision.unwrap_or(6);
    let negative = x.is_sign_negative();
    let body = match spec.conversion {
        'f' => {
            // Rust's `{}` is shortest-round-trip AND positional, which is
            // exactly the digit source Java's `%f` uses.
            let mut text = round_half_up(&format!("{}", x.abs()), precision);
            if spec.group {
                let (whole, frac) = text.split_once('.').unwrap_or((text.as_str(), ""));
                text = if frac.is_empty() {
                    group(whole)
                } else {
                    format!("{}.{}", group(whole), frac)
                };
            }
            text
        }
        _ => {
            let lower = format!("{:e}", x.abs());
            let (mantissa, exponent) = lower.split_once('e').unwrap_or((lower.as_str(), "0"));
            let exponent: i32 = exponent.parse().unwrap_or(0);
            let mantissa = round_half_up(mantissa, precision);
            // A HALF_UP carry can push `9.99` to `10.00`, which moves the
            // exponent.
            let (mantissa, exponent) = if mantissa.starts_with("10") {
                (
                    round_half_up(&format!("{}", 1.0_f64), precision),
                    exponent + 1,
                )
            } else {
                (mantissa, exponent)
            };
            let marker = if spec.conversion == 'E' { 'E' } else { 'e' };
            format!(
                "{mantissa}{marker}{}{:02}",
                if exponent < 0 { '-' } else { '+' },
                exponent.abs()
            )
        }
    };
    let signed = if negative {
        if spec.parens {
            format!("({body})")
        } else {
            format!("-{body}")
        }
    } else if spec.plus {
        format!("+{body}")
    } else if spec.space {
        format!(" {body}")
    } else {
        body
    };
    Ok(pad(spec, signed, true))
}

/// `BigDecimal.setScale(precision, HALF_UP)` over a positional decimal string.
fn round_half_up(text: &str, precision: usize) -> String {
    let (whole, frac) = text.split_once('.').unwrap_or((text, ""));
    let mut digits: Vec<u8> = whole
        .bytes()
        .chain(frac.bytes())
        .map(|b| b - b'0')
        .collect();
    let point = whole.len();
    let keep = point + precision;
    if keep < digits.len() {
        // HALF_UP: >= 5 rounds away from zero, always.
        let carry = digits[keep] >= 5;
        digits.truncate(keep);
        if carry {
            let mut at = keep;
            loop {
                if at == 0 {
                    digits.insert(0, 1);
                    return assemble(&digits, point + 1, precision);
                }
                at -= 1;
                if digits[at] == 9 {
                    digits[at] = 0;
                } else {
                    digits[at] += 1;
                    break;
                }
            }
        }
    } else {
        digits.resize(keep, 0);
    }
    assemble(&digits, point, precision)
}

fn assemble(digits: &[u8], point: usize, precision: usize) -> String {
    let mut out = String::new();
    for (at, d) in digits.iter().enumerate() {
        if at == point && precision > 0 {
            out.push('.');
        }
        out.push((b'0' + d) as char);
    }
    if out.is_empty() {
        out.push('0');
    }
    out
}

/// `,` — three-digit groups, from the right.
fn group(digits: &str) -> String {
    let mut out = String::new();
    for (at, c) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `.N` on a string conversion truncates; on a numeric one it is the fraction
/// width and is handled there.
fn truncate(spec: &Spec, text: String) -> String {
    match spec.precision {
        Some(n) if text.chars().count() > n => text.chars().take(n).collect(),
        _ => text,
    }
}

/// Width, with `-` for left-justify and `0` for zero-fill after the sign.
fn pad(spec: &Spec, text: String, numeric: bool) -> String {
    let Some(width) = spec.width else {
        return text;
    };
    let len = text.chars().count();
    if len >= width {
        return text;
    }
    let fill = width - len;
    if spec.left {
        return format!("{text}{}", " ".repeat(fill));
    }
    if spec.zero && numeric {
        let sign_len = usize::from(text.starts_with(['-', '+', ' ']));
        let (sign, body) = text.split_at(sign_len);
        return format!("{sign}{}{body}", "0".repeat(fill));
    }
    format!("{}{text}", " ".repeat(fill))
}
