// kestra: io.pebbletemplates.pebble.extension.core.NumberFormatFilter -> java.text.DecimalFormat

//! `java.text.DecimalFormat`, for Pebble's `numberformat` filter (package
//! #5b Task B4, spec D13).
//!
//! `NumberFormatFilter.apply` (read from the pinned Pebble 4.1.2 jar with
//! `javap -c -p`) is `new DecimalFormat(format, new
//! DecimalFormatSymbols(locale)).format(input)` when a `format` argument is
//! given and `NumberFormat.getInstance(locale).format(input)` when it is
//! not. Both end in `Format.format(Object)`, which is **not** the double fast
//! path (`DontCareFieldPosition` is never passed), so everything below ports
//! the general path of JDK 21's `DecimalFormat` and `DigitList` source
//! (`applyPattern`, `format(double|long|BigDecimal|BigInteger)`,
//! `subformatNumber`'s non-exponential branch, `DigitList.set`/`round`/
//! `shouldRoundUp` under the default `HALF_EVEN`).
//!
//! # The locale is pinned to `en`
//!
//! Kestra passes the JVM's default locale; egret pins `en` (spec D13, the
//! same choice egret's `date` filter makes when no locale is given): `,`
//! groups, `.` is the decimal point, `-` the minus sign, `%`/`‰` the percent
//! and per-mille signs, `NaN` and `∞` the two non-finite values. A Kestra
//! running under a locale whose symbols differ (`de`, `fr`, `vi`) writes
//! other characters; the corpus was measured on an `en_VN` JVM, whose symbols
//! are `en`'s, so no golden entry exercises the difference.
//!
//! # What is refused, by name
//!
//! Scientific notation (an `E` in the pattern) and the currency sign (`¤`)
//! are refused with an egret message naming them, rather than answering a
//! number that differs: the first needs `DecimalFormat`'s exponent branch,
//! the second a currency, and no corpus entry or flow in the executor corpus
//! uses either.
//!
//! # One approximation, stated
//!
//! For a `double`, `DigitList` starts from `FloatingDecimal`'s digit string
//! and two flags (were the digits rounded up, are they exact). This port
//! takes the **shortest round-trip** digits (Rust's `{:e}`, the algorithm
//! `Double.toString` uses since JDK 19) and computes both flags exactly
//! against the double's binary value. The old `FloatingDecimal` conversion
//! occasionally emits one more digit than the shortest; that only matters
//! when the pattern asks for that many fraction digits (17 significant
//! digits and more), where egret prints the shorter form.

use num_bigint::BigUint;

/// `DecimalFormat.DOUBLE_INTEGER_DIGITS`: the cap `super.get*IntegerDigits`
/// applies on the `double` and `long` paths.
const DOUBLE_INTEGER_DIGITS: usize = 309;
/// `DecimalFormat.DOUBLE_FRACTION_DIGITS`.
const DOUBLE_FRACTION_DIGITS: usize = 340;
/// `DecimalFormat.MAXIMUM_INTEGER_DIGITS` and `MAXIMUM_FRACTION_DIGITS`.
const MAXIMUM_DIGITS: usize = i32::MAX as usize;

/// A parsed pattern: what `applyPattern` leaves in the `DecimalFormat`'s
/// fields, with the affixes already expanded under the `en` symbols.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DecimalFormat {
    pos_prefix: String,
    pos_suffix: String,
    neg_prefix: String,
    neg_suffix: String,
    multiplier: u32,
    min_int: usize,
    max_int: usize,
    min_frac: usize,
    max_frac: usize,
    /// `isGroupingUsed() && groupingSize != 0`, as the size.
    grouping: Option<usize>,
    decimal_always_shown: bool,
}

/// A number as `Format.format(Object)` dispatches it.
#[derive(Debug, Clone)]
pub(super) enum JavaNumber {
    /// `Double`, `Float` and any `Number` that is none of the others.
    Double(f64),
    /// `Long`/`Integer` and a `BigInteger` under 64 bits (`long_path`), or a
    /// wider `BigInteger`. `magnitude` is plain decimal digits.
    Integer {
        negative: bool,
        magnitude: String,
        long_path: bool,
    },
    /// A `BigDecimal`, as its `toString()` (which may carry an exponent).
    Decimal(String),
}

impl DecimalFormat {
    /// `NumberFormat.getInstance(Locale.ENGLISH)`: the `en` number pattern.
    pub(super) fn english_default() -> Self {
        Self::parse("#,##0.###").expect("the en number pattern is well-formed")
    }

    /// `new DecimalFormat(pattern, symbols)`: `applyPattern(pattern, false)`.
    ///
    /// The error text is Java's `IllegalArgumentException` message where Java
    /// throws one, and egret's own for the two refused features.
    pub(super) fn parse(pattern: &str) -> Result<Self, String> {
        let chars: Vec<char> = pattern.chars().collect();
        let malformed = || format!("Malformed pattern \"{pattern}\"");
        let special =
            |ch: char| format!("Unquoted special character '{ch}' in pattern \"{pattern}\"");

        // The defaults a `DecimalFormat` has before `applyPattern` touches
        // them, which is what an EMPTY pattern keeps: `groupingUsed` true and
        // `groupingSize` 3 are field initialisers.
        let mut out = Self {
            pos_prefix: String::new(),
            pos_suffix: String::new(),
            neg_prefix: String::new(),
            neg_suffix: String::new(),
            multiplier: 1,
            min_int: 1,
            max_int: MAXIMUM_DIGITS,
            min_frac: 0,
            max_frac: 3,
            grouping: Some(3),
            decimal_always_shown: false,
        };
        // Affixes in `applyPattern`'s quoted form (`'%`, `''`), expanded at
        // the end, so the "negative pattern equals positive" test compares
        // what Java compares.
        let (mut pos_prefix, mut pos_suffix) = (String::new(), String::new());
        let (mut neg_prefix, mut neg_suffix) = (String::new(), String::new());
        let mut got_negative = false;

        let mut start = 0usize;
        let mut j = 1i32;
        while j >= 0 && start < chars.len() {
            let mut in_quote = false;
            let mut prefix = String::new();
            let mut suffix = String::new();
            let mut decimal_pos: i64 = -1;
            let mut multiplier = 1u32;
            let (mut digit_left, mut zero_digits, mut digit_right) = (0i64, 0i64, 0i64);
            // A Java `byte`: it wraps at 128, and the wrapped value is what
            // the checks below see.
            let mut grouping_count: i8 = -1;
            let mut phase = 0u8;
            let mut in_suffix = false;

            let mut pos = start;
            while pos < chars.len() {
                let ch = chars[pos];
                let affix = if in_suffix { &mut suffix } else { &mut prefix };
                match phase {
                    0 | 2 => {
                        if in_quote {
                            if ch == '\'' {
                                if chars.get(pos + 1) == Some(&'\'') {
                                    pos += 1;
                                    affix.push_str("''");
                                } else {
                                    in_quote = false;
                                }
                                pos += 1;
                                continue;
                            }
                        } else if matches!(ch, '#' | '0' | ',' | '.') {
                            // From the suffix too: Java goes BACK to the
                            // number phase, so `0x0` is two zero digits and
                            // the suffix `x` (`1` is `01x`, measured).
                            phase = 1;
                            continue; // reprocess in phase 1
                        } else if ch == '\u{a4}' {
                            return Err(format!(
                                "the currency sign in a 'numberformat' pattern is not \
                                 supported here: \"{pattern}\""
                            ));
                        } else if ch == '\'' {
                            if chars.get(pos + 1) == Some(&'\'') {
                                pos += 1;
                                affix.push_str("''");
                            } else {
                                in_quote = true;
                            }
                            pos += 1;
                            continue;
                        } else if ch == ';' {
                            if phase == 0 || j == 0 {
                                return Err(special(ch));
                            }
                            start = pos + 1;
                            pos = chars.len();
                            continue;
                        } else if ch == '%' || ch == '\u{2030}' {
                            if multiplier != 1 {
                                return Err(format!(
                                    "Too many percent/per mille characters in pattern \"{pattern}\""
                                ));
                            }
                            multiplier = if ch == '%' { 100 } else { 1000 };
                            affix.push('\'');
                            affix.push(ch);
                            pos += 1;
                            continue;
                        } else if ch == '-' {
                            affix.push_str("'-");
                            pos += 1;
                            continue;
                        }
                        affix.push(ch);
                        pos += 1;
                    }
                    _ => {
                        if j == 0 {
                            // The negative subpattern only names affixes: its
                            // digits, separators and exponent are skipped.
                            while pos < chars.len() {
                                let c = chars[pos];
                                if matches!(c, '#' | '0' | ',' | '.' | 'E') {
                                    pos += 1;
                                } else {
                                    phase = 2;
                                    in_suffix = true;
                                    break;
                                }
                            }
                            continue;
                        }
                        match ch {
                            '#' => {
                                if zero_digits > 0 {
                                    digit_right += 1;
                                } else {
                                    digit_left += 1;
                                }
                                if grouping_count >= 0 && decimal_pos < 0 {
                                    grouping_count = grouping_count.wrapping_add(1);
                                }
                            }
                            '0' => {
                                if digit_right > 0 {
                                    return Err(format!("Unexpected '0' in pattern \"{pattern}\""));
                                }
                                zero_digits += 1;
                                if grouping_count >= 0 && decimal_pos < 0 {
                                    grouping_count = grouping_count.wrapping_add(1);
                                }
                            }
                            ',' => grouping_count = 0,
                            '.' => {
                                if decimal_pos >= 0 {
                                    return Err(format!(
                                        "Multiple decimal separators in pattern \"{pattern}\""
                                    ));
                                }
                                decimal_pos = digit_left + zero_digits + digit_right;
                            }
                            'E' => {
                                return Err(format!(
                                    "scientific notation in a 'numberformat' pattern is not \
                                     supported here: \"{pattern}\""
                                ));
                            }
                            _ => {
                                phase = 2;
                                in_suffix = true;
                                continue;
                            }
                        }
                        pos += 1;
                    }
                }
            }

            // "##.###" -> "#0.###", ".###" -> ".0##".
            if zero_digits == 0 && digit_left > 0 && decimal_pos >= 0 {
                let n = if decimal_pos == 0 { 1 } else { decimal_pos };
                digit_right = digit_left - n;
                digit_left = n - 1;
                zero_digits = 1;
            }

            if (decimal_pos < 0 && digit_right > 0)
                || (decimal_pos >= 0
                    && (decimal_pos < digit_left || decimal_pos > digit_left + zero_digits))
                || grouping_count == 0
                || in_quote
            {
                return Err(malformed());
            }

            if j == 1 {
                pos_prefix = prefix;
                pos_suffix = suffix;
                neg_prefix = pos_prefix.clone();
                neg_suffix = pos_suffix.clone();
                let total = digit_left + zero_digits + digit_right;
                let effective_decimal_pos = if decimal_pos >= 0 { decimal_pos } else { total };
                out.min_int = usize::try_from(effective_decimal_pos - digit_left).unwrap_or(0);
                out.max_int = MAXIMUM_DIGITS;
                out.max_frac = if decimal_pos >= 0 {
                    usize::try_from(total - decimal_pos).unwrap_or(0)
                } else {
                    0
                };
                out.min_frac = if decimal_pos >= 0 {
                    usize::try_from(digit_left + zero_digits - decimal_pos).unwrap_or(0)
                } else {
                    0
                };
                out.grouping = (grouping_count > 0).then_some(grouping_count as usize);
                out.multiplier = multiplier;
                out.decimal_always_shown = decimal_pos == 0 || decimal_pos == total;
            } else {
                neg_prefix = prefix;
                neg_suffix = suffix;
                got_negative = true;
            }
            j -= 1;
        }

        if chars.is_empty() {
            out.min_int = 0;
            out.max_int = MAXIMUM_DIGITS;
            out.min_frac = 0;
            out.max_frac = MAXIMUM_DIGITS;
        }

        if !got_negative || (neg_prefix == pos_prefix && neg_suffix == pos_suffix) {
            neg_suffix = pos_suffix.clone();
            neg_prefix = format!("'-{pos_prefix}");
        }

        out.pos_prefix = expand_affix(&pos_prefix);
        out.pos_suffix = expand_affix(&pos_suffix);
        out.neg_prefix = expand_affix(&neg_prefix);
        out.neg_suffix = expand_affix(&neg_suffix);
        Ok(out)
    }

    /// `Format.format(Object)` on a number.
    pub(super) fn format(&self, number: &JavaNumber) -> String {
        match number {
            JavaNumber::Double(x) => self.format_double(*x),
            JavaNumber::Integer {
                negative,
                magnitude,
                long_path,
            } => {
                let mut digits = DigitList::of_integer(magnitude);
                if self.multiplier != 1 && !digits.is_zero() {
                    digits.decimal_at += if self.multiplier == 100 { 2 } else { 3 };
                }
                // `format(long)` clears the sign of a zero; `signum()` of a
                // zero `BigInteger` is never negative either.
                let negative = *negative && !digits.is_zero();
                let (min_int, max_int, min_frac, max_frac) = if *long_path {
                    self.capped()
                } else {
                    (self.min_int, self.max_int, self.min_frac, self.max_frac)
                };
                self.subformat(
                    &digits, negative, true, min_int, max_int, min_frac, max_frac,
                )
            }
            JavaNumber::Decimal(text) => {
                let (negative, unsigned) = match text.strip_prefix('-') {
                    Some(rest) => (true, rest),
                    None => (false, text.as_str()),
                };
                let mut digits = DigitList::parse(unsigned);
                let negative = negative && !digits.is_zero();
                if self.multiplier != 1 && !digits.is_zero() {
                    digits.decimal_at += if self.multiplier == 100 { 2 } else { 3 };
                }
                digits.round_fixed(self.max_frac, false, true);
                self.subformat(
                    &digits,
                    negative,
                    false,
                    self.min_int,
                    self.max_int,
                    self.min_frac,
                    self.max_frac,
                )
            }
        }
    }

    /// `super.get*Digits()`: the `double`/`long` path's caps.
    fn capped(&self) -> (usize, usize, usize, usize) {
        (
            self.min_int.min(DOUBLE_INTEGER_DIGITS),
            self.max_int.min(DOUBLE_INTEGER_DIGITS),
            self.min_frac.min(DOUBLE_FRACTION_DIGITS),
            self.max_frac.min(DOUBLE_FRACTION_DIGITS),
        )
    }

    fn format_double(&self, number: f64) -> String {
        if number.is_nan() {
            return "NaN".to_string();
        }
        let negative = number < 0.0 || (number == 0.0 && number.is_sign_negative());
        let number = if self.multiplier == 1 {
            number
        } else {
            number * f64::from(self.multiplier)
        };
        if number.is_infinite() {
            let (prefix, suffix) = self.affixes(negative);
            return format!("{prefix}\u{221e}{suffix}");
        }
        let number = number.abs();
        let (min_int, max_int, min_frac, max_frac) = self.capped();
        let shortest = format!("{number:e}");
        let (rounded_up, exact) = shortest_vs_exact(&shortest, number);
        let mut digits = DigitList::parse(&shortest);
        digits.round_fixed(max_frac, rounded_up, exact);
        self.subformat(
            &digits, negative, false, min_int, max_int, min_frac, max_frac,
        )
    }

    fn affixes(&self, negative: bool) -> (&str, &str) {
        if negative {
            (&self.neg_prefix, &self.neg_suffix)
        } else {
            (&self.pos_prefix, &self.pos_suffix)
        }
    }

    /// `subformat` with `subformatNumber`'s non-exponential branch.
    #[allow(clippy::too_many_arguments)]
    fn subformat(
        &self,
        digits: &DigitList,
        negative: bool,
        is_integer: bool,
        min_int: usize,
        max_int: usize,
        min_frac: usize,
        max_frac: usize,
    ) -> String {
        let (prefix, suffix) = self.affixes(negative);
        let mut out = String::from(prefix);
        // `if (digitList.isZero()) digitList.decimalAt = 0`.
        let decimal_at = if digits.is_zero() {
            0
        } else {
            digits.decimal_at
        };
        let count = digits.digits.len();

        let mut int_count = min_int as i64;
        let mut index: usize = 0;
        if decimal_at > 0 && int_count < decimal_at {
            int_count = decimal_at;
        }
        if int_count > max_int as i64 {
            int_count = max_int as i64;
            index = usize::try_from(decimal_at - int_count).unwrap_or(0);
        }
        let before_integer = out.len();
        let mut i = int_count - 1;
        while i >= 0 {
            if i < decimal_at && index < count {
                out.push(char::from(b'0' + digits.digits[index]));
                index += 1;
            } else {
                out.push('0');
            }
            if let Some(size) = self.grouping {
                if i > 0 && i % size as i64 == 0 {
                    out.push(',');
                }
            }
            i -= 1;
        }

        let fraction_present = min_frac > 0 || (!is_integer && index < count);
        if !fraction_present && out.len() == before_integer {
            out.push('0');
        }
        if self.decimal_always_shown || fraction_present {
            out.push('.');
        }
        for i in 0..max_frac {
            if i >= min_frac && (is_integer || index >= count) {
                break;
            }
            if -1 - (i as i64) > decimal_at - 1 {
                out.push('0');
                continue;
            }
            if !is_integer && index < count {
                out.push(char::from(b'0' + digits.digits[index]));
                index += 1;
            } else {
                out.push('0');
            }
        }
        out.push_str(suffix);
        out
    }
}

/// `expandAffix` under the `en` symbols: `'` before a special character
/// stands for its symbol, and `''` for a quote.
fn expand_affix(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c == '\'' {
            if let Some(next) = chars.next() {
                // `%`, `‰` and `-` expand to `en`'s own symbol for each, which
                // is the same character; `''` is a quote.
                out.push(next);
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// `DigitList`: significant decimal digits (no leading zero) and where the
/// point falls among them.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DigitList {
    digits: Vec<u8>,
    decimal_at: i64,
}

impl DigitList {
    fn is_zero(&self) -> bool {
        self.digits.iter().all(|d| *d == 0)
    }

    /// `set(isNegative, long|BigInteger, 0)`: the digits, trailing zeros
    /// dropped, the point after the last.
    fn of_integer(magnitude: &str) -> Self {
        let trimmed = magnitude.trim_start_matches('0');
        let mut digits: Vec<u8> = trimmed.bytes().map(|b| b - b'0').collect();
        let decimal_at = digits.len() as i64;
        while digits.last() == Some(&0) {
            digits.pop();
        }
        Self { digits, decimal_at }
    }

    /// The parsing half of `DigitList.set(boolean, String, ...)`: a plain or
    /// exponent decimal string, unsigned.
    fn parse(text: &str) -> Self {
        let mut digits = Vec::new();
        let mut decimal_at: i64 = -1;
        let mut exponent: i64 = 0;
        let mut leading_zeros_after_point: i64 = 0;
        let mut nonzero_seen = false;
        let chars = text.char_indices();
        for (at, c) in chars {
            match c {
                '.' => decimal_at = digits.len() as i64,
                'e' | 'E' => {
                    // Not reachable today: every BigDecimal source caps the
                    // scale at about 1 MiB of digits (`big_decimal_strings`),
                    // so the exponent fits. A new source with a wider exponent
                    // must not land on the silent `0` (fix wave #5b, M10).
                    let parsed = text[at + 1..].parse::<i64>();
                    debug_assert!(parsed.is_ok(), "an exponent past i64: {text}");
                    exponent = parsed.unwrap_or(0);
                    break;
                }
                _ => {
                    let d = c.to_digit(10).unwrap_or(0) as u8;
                    if !nonzero_seen {
                        nonzero_seen = d != 0;
                        if !nonzero_seen && decimal_at != -1 {
                            leading_zeros_after_point += 1;
                        }
                    }
                    if nonzero_seen {
                        digits.push(d);
                    }
                }
            }
        }
        if decimal_at == -1 {
            decimal_at = digits.len() as i64;
        }
        if nonzero_seen {
            decimal_at += exponent - leading_zeros_after_point;
        }
        Self { digits, decimal_at }
    }

    /// The fixed-point tail of `DigitList.set`: underflow to zero, the
    /// single-digit carry, trailing zeros, then `round`.
    fn round_fixed(&mut self, max_frac: usize, rounded_up: bool, exact: bool) {
        let max = max_frac as i64;
        if -self.decimal_at > max {
            self.digits.clear();
            return;
        }
        if -self.decimal_at == max {
            if self.should_round_up(0, rounded_up, exact) {
                self.digits = vec![1];
                self.decimal_at += 1;
            } else {
                self.digits.clear();
            }
            return;
        }
        while self.digits.len() > 1 && self.digits.last() == Some(&0) {
            self.digits.pop();
        }
        let keep = max.saturating_add(self.decimal_at);
        if keep >= 0 && (keep as usize) < self.digits.len() {
            let mut keep = keep as usize;
            if self.should_round_up(keep, rounded_up, exact) {
                loop {
                    if keep == 0 {
                        // All nines: one digit of one, the point moves.
                        self.digits = vec![1];
                        self.decimal_at += 1;
                        keep = 1;
                        break;
                    }
                    keep -= 1;
                    self.digits[keep] += 1;
                    if self.digits[keep] <= 9 {
                        keep += 1;
                        break;
                    }
                }
            }
            self.digits.truncate(keep);
            while self.digits.len() > 1 && self.digits.last() == Some(&0) {
                self.digits.pop();
            }
        }
    }

    /// `shouldRoundUp` under `HALF_EVEN`.
    fn should_round_up(&self, at: usize, already_rounded: bool, exact: bool) -> bool {
        let count = self.digits.len();
        if at >= count {
            return false;
        }
        let d = self.digits[at];
        if d > 5 {
            return true;
        }
        if d < 5 {
            return false;
        }
        if at == count - 1 {
            if already_rounded {
                return false;
            }
            if !exact {
                return true;
            }
            return at > 0 && !self.digits[at - 1].is_multiple_of(2);
        }
        self.digits[at + 1..].iter().any(|d| *d != 0)
    }
}

/// `FloatingDecimal`'s two flags for `shortest` (a finite, non-negative
/// double's digits in `d.ddde±x` form): were the digits rounded **up** from
/// the binary value, and are they the binary value exactly.
fn shortest_vs_exact(shortest: &str, value: f64) -> (bool, bool) {
    if value == 0.0 {
        return (false, true);
    }
    let (mantissa, exponent) = shortest.split_once('e').unwrap_or((shortest, "0"));
    let exponent: i64 = exponent.parse().unwrap_or(0);
    let (int_part, frac_part) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = format!("{int_part}{frac_part}");
    // shortest = D * 10^q
    let q = exponent - frac_part.len() as i64;
    let d = digits.parse::<BigUint>().unwrap_or_default();

    // value = m * 2^k
    let bits = value.to_bits();
    let raw_exp = ((bits >> 52) & 0x7ff) as i64;
    let fraction = bits & ((1u64 << 52) - 1);
    let (m, k) = if raw_exp == 0 {
        (fraction, -1074)
    } else {
        (fraction | (1u64 << 52), raw_exp - 1075)
    };
    let mut lhs = d;
    let mut rhs = BigUint::from(m);
    let ten = BigUint::from(10u32);
    if q >= 0 {
        lhs *= ten.pow(q as u32);
    } else {
        rhs *= ten.pow((-q) as u32);
    }
    if k >= 0 {
        rhs <<= k as usize;
    } else {
        lhs <<= (-k) as usize;
    }
    (lhs > rhs, lhs == rhs)
}

#[cfg(test)]
mod tests {
    use super::{DecimalFormat, JavaNumber};

    fn fmt(pattern: &str, number: JavaNumber) -> String {
        DecimalFormat::parse(pattern)
            .unwrap_or_else(|e| panic!("{pattern}: {e}"))
            .format(&number)
    }

    fn long(n: i64) -> JavaNumber {
        JavaNumber::Integer {
            negative: n < 0,
            magnitude: n.unsigned_abs().to_string(),
            long_path: true,
        }
    }

    /// Every row measured on JDK 21 (`jshell`, `new DecimalFormat(p,
    /// DecimalFormatSymbols.getInstance(Locale.ENGLISH)).format(x)`), the
    /// first five also through Kestra (`filter-numberformat*`).
    #[test]
    fn decimal_format_answers_as_java() {
        let d = JavaNumber::Double;
        let dec = |s: &str| JavaNumber::Decimal(s.to_string());
        for (pattern, number, want) in [
            ("#,##0.00", d(1234.5678), "1,234.57"),
            ("#,##0.00", long(1_234_567), "1,234,567.00"),
            ("#,##0.00", d(-1234.5), "-1,234.50"),
            ("0.00", d(0.125), "0.12"),
            ("#%", d(0.256), "26%"),
            ("0.###", d(1.5), "1.5"),
            ("#,##0.00", dec("1234.5678"), "1,234.57"),
            // HALF_EVEN on the binary value: 0.135 is below the tie, 0.375 is
            // the tie and 7 is odd.
            ("0.00", d(0.135), "0.14"),
            ("0.00", d(0.375), "0.38"),
            ("0.00", d(1.005), "1.00"),
            ("0.0", d(0.05), "0.1"),
            ("0", d(0.5), "0"),
            ("0", d(1.5), "2"),
            ("0", d(2.5), "2"),
            ("0.00", d(9.999), "10.00"),
            // The sign of a value that rounds to zero is kept.
            ("0.00", d(-0.001), "-0.00"),
            ("0", d(-0.0), "-0"),
            ("0", long(0), "0"),
            // No `0` in the pattern: "#.##" is "#0.##", "#" has no minimum.
            ("#.##", d(0.5), "0.5"),
            (".##", d(0.5), ".5"),
            ("#", d(0.0), "0"),
            ("#", d(0.001), "0"),
            ("0.", d(1.0), "1."),
            ("00000", long(42), "00042"),
            ("#,##,###", long(123_456_789), "123,456,789"),
            ("#,##", long(123_456_789), "1,23,45,67,89"),
            ("'#'0", long(5), "#5"),
            ("0' o''clock'", long(5), "5 o'clock"),
            ("0;(0)", long(-5), "(5)"),
            ("0;0", long(-5), "-5"),
            ("0\u{2030}", d(0.0125), "12\u{2030}"),
            ("0%", long(3), "300%"),
            ("0.00%", dec("0.12345"), "12.34%"),
            ("0.0", dec("1E+3"), "1000.0"),
            ("0.000", dec("-0.0005"), "-0.000"),
            ("0.000", dec("0.0015"), "0.002"),
            ("0.000", dec("0.0025"), "0.002"),
            ("", long(1_234_567), "1,234,567"),
            ("", d(1234.5678), "1,234.5678"),
            ("#,##0.###", d(f64::INFINITY), "\u{221e}"),
            ("#,##0.###", d(f64::NEG_INFINITY), "-\u{221e}"),
            ("#,##0.###", d(f64::NAN), "NaN"),
            ("0.00", d(1e20), "100000000000000000000.00"),
            ("0.####################", d(0.1), "0.1"),
            ("0", d(123_456_789.0), "123456789"),
            ("0x0", long(1), "01x"),
            ("0x0", long(12), "12x"),
            ("0.0x#", d(1.25), "1.25x"),
        ] {
            assert_eq!(
                fmt(pattern, number.clone()),
                want,
                "{pattern} on {number:?}"
            );
        }
        assert_eq!(
            DecimalFormat::english_default().format(&d(1234.5678)),
            "1,234.568"
        );
        assert_eq!(
            DecimalFormat::english_default().format(&d(0.123456)),
            "0.123"
        );
    }

    #[test]
    fn a_pattern_java_refuses_is_refused() {
        for (pattern, want) in [
            ("#,", "Malformed pattern \"#,\""),
            ("0.0.0", "Multiple decimal separators in pattern \"0.0.0\""),
            ("#.#0", "Malformed pattern \"#.#0\""),
            ("0.#0", "Unexpected '0' in pattern \"0.#0\""),
            (
                "0%%",
                "Too many percent/per mille characters in pattern \"0%%\"",
            ),
            (";0", "Unquoted special character ';' in pattern \";0\""),
            (
                "0;0;0",
                "Unquoted special character ';' in pattern \"0;0;0\"",
            ),
            ("'0", "Malformed pattern \"'0\""),
            ("0.00E0", "scientific notation"),
            ("\u{a4}0", "currency sign"),
        ] {
            let error = DecimalFormat::parse(pattern).expect_err(pattern);
            assert!(error.contains(want), "{pattern}: {error}");
        }
    }
}
