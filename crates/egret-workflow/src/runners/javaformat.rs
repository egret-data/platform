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
//!
//! # One divergence this module cannot close
//!
//! Width and precision count **UTF-16 code units** here, as `String.length()`
//! and `String.substring` do — but a precision that would cut a supplementary
//! character in half produces a lone surrogate in Java, which Rust cannot hold
//! in a `String` at all. egret stops one character short instead. The same
//! branch `super::filters::first` documents, and for the same reason it can
//! have no corpus entry: the golden would need an unpaired surrogate escape.
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
//! # The flag grammar is validated, and the first cut validated NONE of it
//!
//! `parse_spec` accepted any flag before any conversion: no per-conversion
//! set, no duplicate check, no mutual exclusion, no width requirement. Over a
//! 3 994-case grid of flag x conversion x width x argument, **1 748 cases
//! Kestra refuses were answered** — and 765 of those were a refusal this
//! module *deleted*, because the wrapper it replaced had refused them for its
//! own reasons. `super::filters::strict_undefined`'s doc, written in the same
//! round, says converting a refusal into an answer is the one divergence this
//! crate's rules do not allow.
//!
//! [`LEGAL_FLAGS`] and [`validate`] carry Java's rules, every one measured
//! rather than read: the per-conversion legal set, duplicates first, `+`/` `
//! and `-`/`0` mutually exclusive, `-`/`0` requiring a width, and precision
//! refused on the integer conversions and on `%`. Three flags on the radix
//! conversions (`+`, ` `, `(`) are checked at PRINT time rather than parse
//! time, which a null argument never reaches — so `'%+x' | format(null)` is
//! `null` and `'%+x' | format(42)` raises. Also measured.
//!
//! **An earlier version of this doc claimed a 519-case grid with "the only
//! disagreements are the four refused conversions".** That grid had no flag
//! axis; the sentence was false by 1 189 cases. The claim is now the 3 994-case
//! grid, and `the_flag_grid_accepts_exactly_what_java_accepts` pins the
//! accept-set as an exact count rather than a threshold.
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

impl Spec {
    /// The flags that were actually given, in Java's own declaration order.
    fn flags(&self) -> Vec<char> {
        let mut out = Vec::new();
        for (present, flag) in [
            (self.left, '-'),
            (self.alt, '#'),
            (self.plus, '+'),
            (self.space, ' '),
            (self.zero, '0'),
            (self.group, ','),
            (self.parens, '('),
        ] {
            if present {
                out.push(flag);
            }
        }
        out
    }
}

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
        // Java's `MissingFormatArgumentException` names the whole specifier,
        // so `%0$s` and `%3$s` are two messages and not one -- they used to
        // collapse together through a `usize::MAX` index.
        let source: String = bytes[at..after].iter().collect();
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
            return Err(pebble_error(format!("Format specifier '{source}'")));
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
    let mut duplicates: Vec<char> = Vec::new();
    while at < chars.len() {
        let flag = chars[at];
        let already = match flag {
            '-' => std::mem::replace(&mut spec.left, true),
            '+' => std::mem::replace(&mut spec.plus, true),
            ' ' => std::mem::replace(&mut spec.space, true),
            '0' => std::mem::replace(&mut spec.zero, true),
            ',' => std::mem::replace(&mut spec.group, true),
            '(' => std::mem::replace(&mut spec.parens, true),
            '#' => std::mem::replace(&mut spec.alt, true),
            _ => break,
        };
        if already {
            duplicates.push(flag);
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
    at += 1;
    // **The `t`/`T` family is TWO characters**, and consuming one left the
    // second as literal text: `'%tY' | format(null)` rendered `nullY` where
    // Kestra renders `null`. The family is refused, but it has to be parsed
    // before it can be refused.
    if matches!(spec.conversion, 't' | 'T') && at < chars.len() {
        at += 1;
    }
    validate(&spec, &duplicates)?;
    Ok((spec, at))
}

/// Every flag `java.util.Formatter.Conversion` allows, per conversion.
///
/// **Measured, not remembered**: a 3 994-case grid of flag x conversion x
/// width x argument through the harness, classified by the exception Java
/// raises. The first cut validated *nothing* — any flag before any
/// conversion — and answered **1 748 cases Kestra refuses**, which is the
/// over-acceptance this crate's rules forbid and which
/// `super::filters::strict_undefined`'s own doc, written in the same round,
/// says is not allowed.
///
/// `n` is absent on purpose: it takes no flags, no width and no precision.
const LEGAL_FLAGS: &[(char, &str)] = &[
    ('s', "-"),
    ('S', "-"),
    ('b', "-"),
    ('B', "-"),
    ('d', "-+ 0,("),
    // `+`, ` ` and `(` are absent from Java's PARSE-time check for the radix
    // conversions and are refused inside `printInteger` instead -- which a
    // **null** argument never reaches, because a null takes the string path.
    // So `'%+x' | format(null)` is `null` and `'%+x' | format(42)` raises.
    // Measured, both. `,` is parse-time and stays here.
    ('o', "-#0+ ("),
    ('x', "-#0+ ("),
    ('X', "-#0+ ("),
    ('f', "-#+ 0,("),
    ('e', "-#+ 0("),
    ('E', "-#+ 0("),
    ('%', "-"),
];

/// The six cross-checks Java runs on a specifier, in the order it runs them.
///
/// Derived from the same grid: `%--s` is a `DuplicateFormatFlags` and not a
/// `MissingFormatWidth`, so duplicates come first; `%+ s` is a
/// `FormatFlagsConversionMismatch` and `%+ d` an `IllegalFormatFlags`, so the
/// per-conversion set is checked before the mutually-exclusive pairs.
fn validate(spec: &Spec, duplicates: &[char]) -> Result<(), MjError> {
    if let Some(flag) = duplicates.first() {
        return Err(pebble_error(format!("Flags = '{flag}'")));
    }
    let flags = spec.flags();
    if spec.conversion == 'n' {
        if let Some(flag) = flags.first() {
            return Err(pebble_error(format!("Flags = '{flag}'")));
        }
        if spec.width.is_some() {
            return Err(pebble_error("Width = ".to_string()));
        }
        if spec.precision.is_some() {
            return Err(pebble_error("Precision = ".to_string()));
        }
        return Ok(());
    }
    if let Some((_, legal)) = LEGAL_FLAGS.iter().find(|(c, _)| *c == spec.conversion) {
        if let Some(flag) = flags.iter().find(|f| !legal.contains(**f)) {
            // `%` is `IllegalFormatFlags` where every other conversion is a
            // `FormatFlagsConversionMismatch`; both are refusals and the
            // wording follows Java's.
            return Err(pebble_error(if spec.conversion == '%' {
                format!("Flags = '{flag}'")
            } else {
                format!("Conversion = {}, Flags = {flag}", spec.conversion)
            }));
        }
    }
    if spec.plus && spec.space {
        return Err(pebble_error("Flags = '+ '"));
    }
    if spec.left && spec.zero {
        return Err(pebble_error("Flags = '-0'"));
    }
    if (spec.left || spec.zero) && spec.width.is_none() {
        return Err(pebble_error(format!("Conversion = {}", spec.conversion)));
    }
    // Precision is a fraction width for `f`/`e`/`E` and a truncation for the
    // string conversions; on an integer or on `%` it is an error.
    if spec.precision.is_some() && matches!(spec.conversion, 'd' | 'o' | 'x' | 'X' | '%') {
        return Err(pebble_error(format!("Precision = {:?}", spec.precision)));
    }
    Ok(())
}

/// One argument through one conversion.
fn render(spec: &Spec, argument: &MjValue) -> Result<String, MjError> {
    // **The refusal comes FIRST, before the null arm.** With the null arm in
    // front, `'%tY' | format(null)` answered `null` for a conversion egret
    // does not implement -- an answer where Kestra also answers, but by
    // accident and only for one argument.
    if matches!(
        spec.conversion,
        'c' | 'C' | 'g' | 'G' | 'a' | 'A' | 'h' | 'H' | 't' | 'T'
    ) {
        return Err(pebble_error(format!(
            "the 'format' filter's '%{}' conversion is not implemented in egret; \
             's S b B d o x X f e E % n' are",
            spec.conversion
        )));
    }
    // **An UNDEFINED argument is not a null.** `is_java_null` folds the two,
    // and this is the argument side of exactly the regression
    // `super::filters::strict_undefined` fixed on the input side:
    // `{{ '%s' | format(nothere) }}` rendered the four characters `null`
    // where Kestra raises `RootAttributeNotFoundException`.
    if argument.kind() == ValueKind::Undefined {
        return Err(pebble_error(
            "an undefined value reached the 'format' filter's arguments: Kestra renders \
             under strictVariables(true), where a missing root attribute or a missing \
             attribute raises rather than becoming null",
        ));
    }
    let null = is_java_null(argument);
    match spec.conversion {
        // **Three arms, not two.** Java's `%b` is `null -> false`, a
        // **Boolean -> its own value**, anything else -> `true`. The middle
        // one was missing, so `'%b' | format(false)` rendered `true`. The only
        // `%b` entry the first cut had passed a null -- the one argument for
        // which two arms are right.
        'b' | 'B' => {
            let text = if null {
                "false".to_string()
            } else if argument.kind() == ValueKind::Bool {
                kestra_to_string(argument)
            } else {
                "true".to_string()
            };
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
        // A conversion character Java does not have either. The ones it has
        // and egret does not are refused above, before the null arm.
        other => Err(pebble_error(format!("Conversion = '{other}'"))),
    }
}

fn integer(spec: &Spec, argument: &MjValue) -> Result<String, MjError> {
    // The three flags Java checks in `printInteger` rather than at parse
    // time; see [`LEGAL_FLAGS`].
    if matches!(spec.conversion, 'o' | 'x' | 'X') {
        if let Some(flag) = spec.flags().into_iter().find(|f| "+ (".contains(*f)) {
            return Err(pebble_error(format!(
                "Conversion = {}, Flags = {flag}",
                spec.conversion
            )));
        }
    }
    if argument.kind() != ValueKind::Number || !argument.is_integer() {
        return Err(pebble_error(format!(
            "{} != java.lang.Integer",
            java_class_name(argument)
        )));
    }
    // **`i128`, not `i64`.** `java_class_name` already says `BigInteger` past
    // `i64` and `sort`'s comparator already routes those through `i128`; this
    // was the only consumer that narrowed, so
    // `'%d' | format(18446744073709551615)` was refused where Kestra answers.
    let Ok(n) = i128::try_from(argument.clone()) else {
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
        // Integer/Long/BigInteger split `java_class_name` already makes. A
        // `BigInteger` has no fixed width, so Java prints sign-and-magnitude
        // there rather than a complement.
        radix => {
            let class = java_class_name(argument);
            let raw = match (class.as_str(), radix, negative) {
                ("java.lang.Integer", 'o', _) => format!("{:o}", n as i32 as u32),
                ("java.lang.Integer", _, _) => format!("{:x}", n as i32 as u32),
                ("java.math.BigInteger", 'o', true) => format!("-{:o}", n.unsigned_abs()),
                ("java.math.BigInteger", _, true) => format!("-{:x}", n.unsigned_abs()),
                ("java.math.BigInteger", 'o', false) => format!("{:o}", n as u128),
                ("java.math.BigInteger", _, false) => format!("{:x}", n as u128),
                (_, 'o', _) => format!("{:o}", n as i64 as u64),
                _ => format!("{:x}", n as i64 as u64),
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
            // The `#` prefix sits in the SIGN position for zero fill, so
            // `%#012x` is `0x00ffffff01` and not `000xffffff01`.
            return Ok(pad_with_prefix(spec, prefix, raw));
        }
    };
    // `(` wraps BEFORE the zero fill, so `%(012d` of -255 is `(0000000255)`
    // and not `0000000(255)`. `pad_at` keeps the leading `(` in place.
    // `(` wraps BEFORE the zero fill, so `%(012d` of -255 is `(0000000255)`
    // and not `0000000(255)`. `pad_at` keeps the leading `(` in place.
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
    let Some(n) = spec.precision else {
        return text;
    };
    if utf16_len(&text) <= n {
        return text;
    }
    // `String.substring(0, n)` counts UTF-16 code units, so a supplementary
    // character can be cut in half there. Rust cannot hold half of one, so
    // egret stops one character short instead -- `'%.2s'` of `"\u{1d11e}ab"`
    // is the musical symbol alone on both sides, and the divergence only
    // appears where Java would have produced a lone surrogate, which no JSON
    // golden could carry anyway. Recorded in this module's doc.
    let mut out = String::new();
    let mut units = 0usize;
    for c in text.chars() {
        if units + c.len_utf16() > n {
            break;
        }
        units += c.len_utf16();
        out.push(c);
    }
    out
}

/// Width, with `-` for left-justify and `0` for zero-fill after the sign.
///
/// **The length is in UTF-16 code units**, because Java's `Formatter` measures
/// a `String` with `length()`. `'%6s' | format('\u{1d11e}ab')` pads by two
/// there and by three under a `char` count -- the same branch
/// `super::filters::first` documents, and the first cut of this module
/// repeated it without a note.
fn pad(spec: &Spec, text: String, numeric: bool) -> String {
    let sign_len = if numeric {
        usize::from(text.starts_with(['-', '+', ' ', '(']))
    } else {
        0
    };
    pad_at(spec, text, sign_len, numeric)
}

/// The same, where the fill goes after a `#` radix prefix rather than a sign.
fn pad_with_prefix(spec: &Spec, prefix: &str, raw: String) -> String {
    let text = format!("{prefix}{raw}");
    pad_at(spec, text, prefix.len(), true)
}

fn pad_at(spec: &Spec, text: String, keep: usize, numeric: bool) -> String {
    let Some(width) = spec.width else {
        return text;
    };
    let len = utf16_len(&text);
    if len >= width {
        return text;
    }
    let fill = width - len;
    if spec.left {
        return format!("{text}{}", " ".repeat(fill));
    }
    if spec.zero && numeric {
        let (head, body) = text.split_at(keep);
        return format!("{head}{}{body}", "0".repeat(fill));
    }
    format!("{}{text}", " ".repeat(fill))
}

/// `String.length()`.
fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `String.format(fmt, args)` over `MjValue`s, the way the filter calls it.
    fn f(template: &str, args: &[MjValue]) -> Result<String, String> {
        format(template, args).map_err(|e| e.to_string())
    }

    fn ok(template: &str, args: &[MjValue]) -> String {
        f(template, args).unwrap_or_else(|e| panic!("{template}: {e}"))
    }

    fn err(template: &str, args: &[MjValue]) -> String {
        f(template, args).expect_err("must be refused")
    }

    /// **The grid.** Every flag against every conversion, with and without a
    /// width, against an integer / a double / a string / a boolean / a null.
    ///
    /// This module shipped with **zero** unit tests and 21 corpus entries, not
    /// one of which carried an illegal flag — and five defects shipped green
    /// behind that. A 3 994-case differential then found **1 748 cases where
    /// Kestra raises and egret answered**, of which 765 were a refusal the
    /// round before had *deleted* by replacing a wrapper that refused them for
    /// its own reasons.
    ///
    /// So the shape of that differential lives here, as a property rather than
    /// as examples: for every specifier the grid covers, egret either answers
    /// or refuses, and the **set** of specifiers it accepts is pinned. The
    /// exact values are `oracle/corpus.json`'s job; this is what stops the
    /// accept-set widening again.
    #[test]
    fn the_flag_grid_accepts_exactly_what_java_accepts() {
        let args = [
            MjValue::from(42),
            MjValue::from(1.5),
            MjValue::from("ab"),
            MjValue::from(true),
            MjValue::from(()),
        ];
        // Per conversion, the flags Java allows at PARSE time. Measured
        // through the harness -- see [`LEGAL_FLAGS`].
        let legal: &[(char, &str)] = &[
            ('s', "-"),
            ('S', "-"),
            ('b', "-"),
            ('B', "-"),
            ('d', "-+ 0,("),
            ('o', "-#0+ ("),
            ('x', "-#0+ ("),
            ('X', "-#0+ ("),
            ('f', "-#+ 0,("),
            ('e', "-#+ 0("),
            ('E', "-#+ 0("),
            ('%', "-"),
        ];
        let mut accepted = 0usize;
        let mut refused = 0usize;
        for (conversion, allowed) in legal {
            for flag in ["", "-", "#", "+", " ", "0", ",", "("] {
                for width in ["", "8"] {
                    let spec = format!("%{flag}{width}{conversion}");
                    // A flag outside the conversion's set must be refused for
                    // EVERY argument -- that is the 1 748.
                    let illegal_here = !flag.is_empty() && !allowed.contains(flag);
                    // `-` and `0` need a width wherever they are legal.
                    let needs_width =
                        width.is_empty() && (flag == "-" || (flag == "0" && allowed.contains('0')));
                    for argument in &args {
                        let got = f(&spec, std::slice::from_ref(argument));
                        if illegal_here || needs_width {
                            assert!(
                                got.is_err(),
                                "{spec} with {argument:?} must be refused, got {got:?}"
                            );
                            refused += 1;
                        } else if got.is_ok() {
                            accepted += 1;
                        }
                    }
                }
            }
        }
        // Exact, not a floor: the accept-set is the thing this guards, and a
        // threshold lets it widen. 455 refusals and 405 acceptances over the
        // 12 conversions x 8 flags x 2 widths x 5 arguments this walks.
        assert_eq!(refused, 455, "the refusal set moved");
        assert_eq!(accepted, 229, "the accept set moved");

        // Duplicates, exclusions and the two width rules, each by name.
        assert!(err("%--8s", &args).contains("Flags = '-'"));
        assert!(err("%##8x", &args).contains("Flags = '#'"));
        assert!(err("%+ 8d", &args).contains("Flags = '+ '"));
        assert!(err("%-08d", &args).contains("Flags = '-0'"));
        assert!(err("%-s", &args).contains("Conversion = s"));
        assert!(err("%0d", &args).contains("Conversion = d"));
        assert!(err("%.2d", &args).contains("Precision"));
        assert!(err("%.2%", &args).contains("Precision"));
        assert!(err("%8n", &args).contains("Width"));
        assert!(err("%-n", &args).contains("Flags"));
    }

    /// `%b` is **three** arms, and the middle one was missing.
    ///
    /// The shipped code was `if null { false } else { true }`, so
    /// `'%b' | format(false)` rendered `true` — and the only corpus entry for
    /// `%b` passed a *null*, the one argument for which two arms are right.
    #[test]
    fn the_boolean_conversion_reads_a_boolean() {
        assert_eq!(ok("%b", &[MjValue::from(false)]), "false");
        assert_eq!(ok("%B", &[MjValue::from(false)]), "FALSE");
        assert_eq!(ok("%b", &[MjValue::from(true)]), "true");
        assert_eq!(ok("%b", &[MjValue::from(())]), "false");
        assert_eq!(ok("%B", &[MjValue::from(())]), "FALSE");
        // Anything that is not a Boolean and not null is `true`.
        assert_eq!(ok("%b", &[MjValue::from(0)]), "true");
        assert_eq!(ok("%b", &[MjValue::from("")]), "true");
    }

    /// An **undefined** argument is refused, which is the argument side of the
    /// guard `super::filters::strict_undefined` put on the inputs.
    #[test]
    fn an_undefined_argument_is_refused() {
        let undefined = MjValue::UNDEFINED;
        for spec in ["%s", "%d", "%b", "%x", "%.2f"] {
            let message = err(spec, std::slice::from_ref(&undefined));
            assert!(
                message.contains("an undefined value reached"),
                "{spec}: {message}"
            );
        }
        // A real null still takes the string path.
        assert_eq!(ok("%s", &[MjValue::from(())]), "null");
    }

    /// The integer conversions carry `i128`, not `i64`.
    #[test]
    fn an_integer_past_i64_still_renders() {
        let big = MjValue::from(18_446_744_073_709_551_615u64);
        assert_eq!(ok("%d", std::slice::from_ref(&big)), "18446744073709551615");
        assert_eq!(ok("%x", std::slice::from_ref(&big)), "ffffffffffffffff");
        assert_eq!(
            ok("%,d", std::slice::from_ref(&big)),
            "18,446,744,073,709,551,615"
        );
    }

    /// Zero fill goes **after** the `#` prefix and **inside** the `(` wrap.
    #[test]
    fn the_fill_respects_the_prefix_and_the_parentheses() {
        assert_eq!(ok("%#012x", &[MjValue::from(42)]), "0x000000002a");
        assert_eq!(ok("%#012x", &[MjValue::from(-255)]), "0x00ffffff01");
        assert_eq!(ok("%#012X", &[MjValue::from(-255)]), "0X00FFFFFF01");
        assert_eq!(ok("%(012d", &[MjValue::from(-255)]), "(0000000255)");
        assert_eq!(ok("%012d", &[MjValue::from(-255)]), "-00000000255");
    }

    /// Width and precision count **UTF-16 code units**, as `String.length()`
    /// does.
    #[test]
    fn width_and_precision_count_utf16_code_units() {
        let musical = MjValue::from("\u{1d11e}ab");
        assert_eq!(ok("%6s", std::slice::from_ref(&musical)), "  \u{1d11e}ab");
        assert_eq!(ok("%-6s", std::slice::from_ref(&musical)), "\u{1d11e}ab  ");
        // `.2` is two code units, which is the whole supplementary character.
        assert_eq!(ok("%.2s", std::slice::from_ref(&musical)), "\u{1d11e}");
        // `.3` would be the character plus half of the `a` in Java; egret
        // stops one short rather than emitting a lone surrogate, which it
        // cannot represent. Recorded in this module's doc.
        assert_eq!(ok("%.3s", std::slice::from_ref(&musical)), "\u{1d11e}a");
    }

    /// The `t`/`T` family is two characters, and the refusal comes before the
    /// null arm.
    #[test]
    fn the_date_family_is_parsed_before_it_is_refused() {
        for spec in ["%tY", "%TY", "%8tY"] {
            let message = err(spec, &[MjValue::from(())]);
            assert!(
                message.contains("conversion is not implemented"),
                "{spec}: {message}"
            );
        }
        // Not `nullY`: the second character was falling through as literal
        // text because `parse_spec` consumed only one.
        assert!(!err("%tY", &[MjValue::from(())]).contains("nullY"));
    }

    /// The argument index, including the two out-of-range spellings that used
    /// to collapse into one message through `usize::MAX`.
    #[test]
    fn the_argument_index_names_the_index_it_could_not_find() {
        let args = [MjValue::from("a"), MjValue::from("b")];
        assert_eq!(ok("%2$s-%1$s", &args), "b-a");
        assert_eq!(ok("%1$s-%1$s", &args), "a-a");
        // `%0$s` is Java's `MissingFormatArgumentException` too, and it is a
        // different index from `%3$s`.
        assert!(err("%0$s", &args).contains("%0$s"));
        assert!(err("%3$s", &args).contains("%3$s"));
        assert!(err("%s-%s", &args[..1]).contains("%s"));
    }
}
