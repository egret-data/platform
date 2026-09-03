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

use super::env::{write_java_double, OrderedMap};
use super::jqsandbox;

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

// ---------------------------------------------------------------------------
// The reader (W2 Task 7 — `fromIon`)
// ---------------------------------------------------------------------------

/// Reads an Ion **text** stream into one `minijinja::Value` per top-level
/// value — `FileSerde.readAll(InputStream)`, which is a Jackson
/// `MappingIterator` over `JacksonMapper.ofIon()`.
///
/// # Why this is hand-written too
///
/// The module doc above explains why `ion-rs` cannot produce Kestra's *text*,
/// and that argument is about the writer only. The reader is here for a
/// different reason and it is worth stating rather than inheriting: this
/// workspace builds offline against a vendored registry, `ion-rs` is not in
/// it, and a reader is the smaller half of the format — Kestra's own writer,
/// twenty lines above, defines the surface that has to round-trip.
///
/// # What it does not read, and why each is a refusal rather than a guess
///
/// **Measured against the harness, not assumed.** An earlier version of this
/// doc said "Jackson's `IonParser` drops [annotations], so the value they
/// decorate is what comes back". That is false and it was written without
/// running one: `fromIon('a::1')` is the string `a::1` in Kestra and
/// `fromIon('[a::1, 2]')` throws. The sentence was load-bearing for a whole
/// annotation-skipping loop that silently answered `1` and `[1,2]`.
///
/// | construct | Kestra | egret |
/// |---|---|---|
/// | `a::1` (an annotation) | the string `a::1` | **refused** |
/// | `[a::1, 2]` | throws | **refused** |
/// | `{{ aGVsbG8= }}` (a blob) | `"aGVsbG8="`, the base64 text | **refused** |
/// | `{{ "clob" }}` | `"Y2xvYg=="`, base64 of the bytes | **refused** |
/// | `2024-01-01T00:00:00Z`, `2024T` | `2024-01-01` | **refused** |
///
/// None of them has a `minijinja::Value` that means the same thing, and
/// `write_ion` never emits one, so nothing egret writes can produce them. Each
/// is refused **by name** — the same call `javafmt` makes for the three CLDR
/// pattern letters, and the only kind of divergence this crate's own rule
/// allows. A timestamp in particular would otherwise become a string, which
/// reads back as a different type with no error anywhere.
///
/// S-expressions are **not** on that list any more: Kestra reads `(1 2)` as
/// `[1,2]`, `(a b)` as `["a","b"]` and `(+ 1 2)` as `["+",1,2]`, so this
/// reader does too.
///
/// # One divergence that is not a refusal
///
/// **A decimal is a `Double` here and a `BigDecimal` in Kestra.** `1.` and
/// `0.` render as `1` and `0` there and `1.0`/`0.0` here, `1d3` is `1000`
/// there and `1000.0` here, and `1.50` keeps its trailing zero there and not
/// here. `write_ion` always writes a float (`2.5e0`), so no round trip through
/// this crate reaches it — only hand-written Ion does. Recorded rather than
/// fixed, because carrying `BigDecimal`'s scale means returning
/// `filters::BigNumber` from a second module.
///
/// # Bounded, because a struct nests
///
/// `[[[[…` is author-controlled input and this is a recursive descent over
/// it, on the render thread. The bound is [`jqsandbox::MAX_VALUE_DEPTH`], the
/// crate's one value-depth invariant, rather than a fifth number: *no value
/// anywhere in this crate is more than `MAX_VALUE_DEPTH` deep*.
pub(super) fn read_ion(text: &str) -> Result<Vec<MjValue>, String> {
    let mut reader = IonReader {
        rest: text,
        depth: 0,
        unterminated_comment: false,
    };
    let mut values = Vec::new();
    reader.skip_ignorable();
    // `$ion_1_0` is the version marker, not a value.
    if let Some(rest) = reader.rest.strip_prefix("$ion_1_0") {
        if !starts_identifier_char(rest) {
            reader.rest = rest;
            reader.skip_ignorable();
        }
    }
    while !reader.rest.is_empty() {
        values.push(reader.value()?);
        reader.skip_ignorable();
    }
    if reader.unterminated_comment {
        return Err("unterminated ion block comment".to_string());
    }
    Ok(values)
}

/// Does `s` begin with a character that would continue an identifier?
fn starts_identifier_char(s: &str) -> bool {
    s.chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '$' || c == '_')
}

struct IonReader<'a> {
    rest: &'a str,
    depth: usize,
    /// Set by [`IonReader::skip_ignorable`] when a `/*` has no `*/`.
    unterminated_comment: bool,
}

impl IonReader<'_> {
    fn fail<T>(&self, what: &str) -> Result<T, String> {
        Err(what.to_string())
    }

    /// Whitespace, `//` line comments and `/* */` block comments.
    fn skip_ignorable(&mut self) {
        loop {
            let trimmed = self.rest.trim_start();
            if let Some(after) = trimmed.strip_prefix("//") {
                self.rest = after.find('\n').map_or("", |at| &after[at..]);
                continue;
            }
            if let Some(after) = trimmed.strip_prefix("/*") {
                // An UNTERMINATED block comment is `Corrupt content to decode`
                // in Kestra, not an empty document. Swallowing it to the end
                // of the input made a truncated document render as NOTHING,
                // which is the silent-wrong-answer class this package exists
                // to prevent. A line comment with no trailing newline is fine
                // on both sides — `1 // c` is `1`.
                match after.find("*/") {
                    Some(at) => self.rest = &after[at + 2..],
                    None => {
                        self.unterminated_comment = true;
                        self.rest = "";
                        return;
                    }
                }
                continue;
            }
            self.rest = trimmed;
            return;
        }
    }

    fn peek(&self) -> Option<char> {
        self.rest.chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.rest = &self.rest[c.len_utf8()..];
        Some(c)
    }

    fn value(&mut self) -> Result<MjValue, String> {
        // Annotations (`sym::value`) are refused, not skipped. See this
        // module's doc: Kestra does NOT drop them.
        let save = self.rest;
        if let Ok(Some(_)) = self.symbol_like() {
            let annotated = self.rest.trim_start().starts_with("::");
            self.rest = save;
            if annotated {
                return self.fail(
                    "egret does not read an ion annotation; Kestra answers the annotated \
                     text for a bare one and throws for one inside a container",
                );
            }
        } else {
            self.rest = save;
        }

        match self.peek() {
            None => self.fail("unexpected end of ion"),
            Some('[') => self.list(),
            Some('(') => self.sexp(),
            Some('{') => {
                if self.rest.starts_with("{{") {
                    return self.fail("egret does not read an ion blob or clob");
                }
                self.struct_()
            }
            Some('"') | Some('\'') => {
                let text = self.string_or_symbol()?;
                Ok(MjValue::from(text))
            }
            Some(_) => self.bare(),
        }
    }

    /// An s-expression, which Jackson reads as a **list**: `(1 2)` is `[1,2]`,
    /// `(a b)` is `["a","b"]` and `(+ 1 2)` is `["+",1,2]`. Measured. Elements
    /// are separated by whitespace and there are no commas.
    fn sexp(&mut self) -> Result<MjValue, String> {
        self.enter()?;
        let _ = self.bump();
        let mut items = Vec::new();
        loop {
            self.skip_ignorable();
            match self.peek() {
                None => return self.fail("unclosed ion s-expression"),
                Some(')') => {
                    let _ = self.bump();
                    break;
                }
                Some(_) => items.push(self.sexp_value()?),
            }
        }
        self.depth -= 1;
        Ok(MjValue::from(items))
    }

    /// One s-expression element: anything a value can be, plus the operator
    /// symbols Ion allows only here.
    fn sexp_value(&mut self) -> Result<MjValue, String> {
        if self.peek().is_some_and(is_ion_operator_char) {
            let end = self
                .rest
                .find(|c: char| !is_ion_operator_char(c))
                .unwrap_or(self.rest.len());
            let token = &self.rest[..end];
            // `-1` and `+inf` are values, not operators: a lone sign followed
            // by a digit or a letter belongs to the number after it.
            let is_sign_of_a_value = matches!(token, "-" | "+")
                && self.rest[end..]
                    .starts_with(|c: char| c.is_ascii_digit() || c.is_ascii_alphabetic());
            if !is_sign_of_a_value {
                let token = token.to_string();
                self.rest = &self.rest[end..];
                return Ok(MjValue::from(token));
            }
        }
        self.value()
    }

    fn enter(&mut self) -> Result<(), String> {
        self.depth += 1;
        if self.depth > jqsandbox::MAX_VALUE_DEPTH {
            return Err(format!(
                "the ion value nests more than {} levels deep, which is what \
                 every writer in this crate can walk",
                jqsandbox::MAX_VALUE_DEPTH
            ));
        }
        Ok(())
    }

    fn list(&mut self) -> Result<MjValue, String> {
        self.enter()?;
        let _ = self.bump();
        let mut items = Vec::new();
        loop {
            self.skip_ignorable();
            match self.peek() {
                None => return self.fail("unclosed ion list"),
                Some(']') => {
                    let _ = self.bump();
                    break;
                }
                // No `Some(',') => continue` arm: `[1,,2]` is corrupt in
                // Kestra and an earlier cut of this reader answered `[1,2]`.
                Some(_) => items.push(self.value()?),
            }
            self.skip_ignorable();
            match self.peek() {
                Some(',') => {
                    let _ = self.bump();
                }
                Some(']') => {
                    let _ = self.bump();
                    break;
                }
                _ => return self.fail("unclosed ion list"),
            }
        }
        self.depth -= 1;
        Ok(MjValue::from(items))
    }

    fn struct_(&mut self) -> Result<MjValue, String> {
        self.enter()?;
        let _ = self.bump();
        let mut entries: Vec<(String, MjValue)> = Vec::new();
        loop {
            self.skip_ignorable();
            match self.peek() {
                None => return self.fail("unclosed ion struct"),
                Some('}') => {
                    let _ = self.bump();
                    break;
                }
                // No `Some(',') => continue` arm: `{,}` is corrupt in Kestra.
                Some(_) => {}
            }
            let Some(field) = self.symbol_like()? else {
                return self.fail("an ion field name must be a symbol or a string");
            };
            self.skip_ignorable();
            if self.bump() != Some(':') {
                return self.fail("an ion field name must be followed by ':'");
            }
            self.skip_ignorable();
            let value = self.value()?;
            // Pushed, not searched. Jackson keeps the LAST of two same-named
            // fields the way a `Map.put` does, and `super::env::last_value_wins`
            // — which `OrderedMap::new` runs — is where that now lives for the
            // whole crate. The linear `find` this replaced was O(fields^2):
            // 26 s for an 80 000-field document, against 17 ms for the same
            // width through `fromJson`.
            entries.push((field, value));
            self.skip_ignorable();
            match self.peek() {
                Some(',') => {
                    let _ = self.bump();
                }
                Some('}') => {
                    let _ = self.bump();
                    break;
                }
                _ => return self.fail("unclosed ion struct"),
            }
        }
        self.depth -= 1;
        Ok(MjValue::from_object(OrderedMap::new(entries)))
    }

    /// A field name or an annotation: a quoted symbol, a string, or an
    /// identifier. `None` means the cursor is not on one at all.
    fn symbol_like(&mut self) -> Result<Option<String>, String> {
        match self.peek() {
            Some('\'') | Some('"') => Ok(Some(self.string_or_symbol()?)),
            Some(c) if c.is_ascii_alphabetic() || c == '$' || c == '_' => {
                let end = self
                    .rest
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '$' || c == '_'))
                    .unwrap_or(self.rest.len());
                let name = self.rest[..end].to_string();
                self.rest = &self.rest[end..];
                Ok(Some(name))
            }
            _ => Ok(None),
        }
    }

    /// `"…"`, `'…'` and the triple-quoted long form, which concatenates when
    /// two of them are adjacent.
    fn string_or_symbol(&mut self) -> Result<String, String> {
        if self.rest.starts_with("'''") {
            let mut out = String::new();
            loop {
                self.rest = &self.rest[3..];
                out.push_str(&self.quoted_body("'''")?);
                let save = self.rest;
                self.skip_ignorable();
                if !self.rest.starts_with("'''") {
                    self.rest = save;
                    return Ok(out);
                }
            }
        }
        let quote = match self.bump() {
            Some('"') => "\"",
            Some('\'') => "'",
            _ => return self.fail("expected an ion string"),
        };
        self.quoted_body(quote)
    }

    /// The body of a quoted form, up to the unescaped `terminator`.
    fn quoted_body(&mut self, terminator: &str) -> Result<String, String> {
        let mut out = String::new();
        loop {
            if self.rest.starts_with(terminator) {
                self.rest = &self.rest[terminator.len()..];
                return Ok(out);
            }
            let Some(c) = self.bump() else {
                return self.fail("unterminated ion string");
            };
            if c != '\\' {
                out.push(c);
                continue;
            }
            let Some(escape) = self.bump() else {
                return self.fail("unterminated ion escape");
            };
            match escape {
                '0' => out.push('\0'),
                'a' => out.push('\u{7}'),
                'b' => out.push('\u{8}'),
                't' => out.push('\t'),
                'n' => out.push('\n'),
                'f' => out.push('\u{c}'),
                'r' => out.push('\r'),
                'v' => out.push('\u{b}'),
                '"' | '\'' | '?' | '\\' | '/' => out.push(escape),
                // A backslash-newline is a line continuation: it produces
                // nothing at all.
                '\n' => {}
                'x' => out.push(self.hex_escape(2)?),
                'u' => out.push(self.hex_escape(4)?),
                'U' => out.push(self.hex_escape(8)?),
                other => return Err(format!("unknown ion escape \\{other}")),
            }
        }
    }

    fn hex_escape(&mut self, digits: usize) -> Result<char, String> {
        if self.rest.len() < digits || !self.rest.is_char_boundary(digits) {
            return self.fail("truncated ion hex escape");
        }
        let (hex, rest) = self.rest.split_at(digits);
        self.rest = rest;
        u32::from_str_radix(hex, 16)
            .ok()
            .and_then(char::from_u32)
            .ok_or_else(|| format!("invalid ion hex escape \\x{hex}"))
    }

    /// `null`, `null.<type>`, `true`, `false`, a number, a special float, or
    /// a bare symbol.
    fn bare(&mut self) -> Result<MjValue, String> {
        let end = self
            .rest
            .find(|c: char| {
                c.is_whitespace()
                    // `/` ends a token because a comment may abut one:
                    // `1//c` is `1` in Kestra and was an error here.
                    || matches!(
                        c,
                        ',' | ']' | '}' | ')' | '(' | '[' | '{' | '\'' | '"' | '/'
                    )
            })
            .unwrap_or(self.rest.len());
        let token = &self.rest[..end];
        if token.is_empty() {
            return self.fail("unexpected character in ion");
        }
        self.rest = &self.rest[end..];

        if token == "null" || token.starts_with("null.") {
            return Ok(MjValue::from(()));
        }
        if token == "true" {
            return Ok(MjValue::from(true));
        }
        if token == "false" {
            return Ok(MjValue::from(false));
        }
        match token {
            "nan" => return Ok(MjValue::from(f64::NAN)),
            "+inf" => return Ok(MjValue::from(f64::INFINITY)),
            "-inf" => return Ok(MjValue::from(f64::NEG_INFINITY)),
            _ => {}
        }
        match ion_number(token) {
            Some(Ok(value)) => return Ok(value),
            Some(Err(why)) => return Err(why),
            None => {}
        }
        // A timestamp is a real Ion type with no `minijinja::Value` that
        // means the same thing, and reading it as text would change its type
        // silently. Named rather than guessed.
        if is_ion_timestamp(token) {
            return Err(format!(
                "egret does not read the ion timestamp {token}; there is no \
                 value in this renderer that means the same thing"
            ));
        }
        // `$0`, `$12`: a symbol ID, not a name. Kestra refuses them
        // (`Corrupt content to decode`) and an earlier cut answered `$0`.
        if token.len() > 1
            && token.starts_with('$')
            && token[1..].bytes().all(|b| b.is_ascii_digit())
        {
            return Err(format!("an ion symbol ID is not a value: {token}"));
        }
        if is_ion_identifier(token) || token.starts_with('$') {
            return Ok(MjValue::from(token.to_string()));
        }
        Err(format!("unexpected token in ion: {token}"))
    }
}

/// `2024-01-15`, `2024-01-15T…`, `2024T`, `2024-01T` — Ion's timestamp shapes
/// all start with four digits and a `-` or a `T`.
fn is_ion_timestamp(token: &str) -> bool {
    let bytes = token.as_bytes();
    bytes.len() >= 5
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && (bytes[4] == b'-' || bytes[4] == b'T')
}

/// An Ion int, decimal or float, or `None` if the token is not a number.
///
/// The three families are separated by their exponent letter, not by their
/// shape: `1d3` is a **decimal** and `1e3` is a float, and Ion writes a
/// decimal's exponent with `d` for exactly that reason (which is why
/// `write_ion` above spells `BigDecimal`'s `E` as `d`).
///
/// # Two things Ion forbids that an earlier cut accepted
///
/// Both measured — Kestra answers `Corrupt content to decode` for each, and
/// egret answered `7` and `5`:
///
/// - **a leading `+`**: `+5` and `+1.5` are not numbers. `+inf` is, and is
///   handled by its caller before this is reached.
/// - **a leading zero**: `007`, `00` and `-007` are refused, while `0`, `-0`,
///   `0.5`, `0x10` and `0b11` are fine. The rule is a `0` followed by another
///   *digit*.
///
/// `None` means "not a number", which the caller turns into a symbol or an
/// error; `Some(Err(..))` means "a number, and Ion rejects it".
#[allow(clippy::type_complexity)]
fn ion_number(token: &str) -> Option<Result<MjValue, String>> {
    let (negative, digits) = match token.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, token),
    };
    if let Some(after) = token.strip_prefix('+') {
        // Only reached for a token that otherwise looks numeric; `+inf` and
        // the bare symbol `+` never get here.
        return after
            .starts_with(|c: char| c.is_ascii_digit())
            .then(|| Err(format!("ion forbids a leading '+' on a number: {token}")));
    }
    if digits.is_empty() || !digits.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }

    // Radix forms, which Ion allows `_` inside. Checked before the
    // leading-zero rule, since every one of them starts with `0`.
    for (prefix, radix) in [("0x", 16), ("0X", 16), ("0b", 2), ("0B", 2)] {
        if let Some(rest) = digits.strip_prefix(prefix) {
            let cleaned = rest.replace('_', "");
            if cleaned.is_empty() {
                return Some(Err(format!("not an ion number: {token}")));
            }
            return Some(match i128::from_str_radix(&cleaned, radix) {
                Ok(magnitude) => Ok(int_value(if negative { -magnitude } else { magnitude })),
                Err(_) => Err(format!("ion integer out of range: {token}")),
            });
        }
    }
    if digits.starts_with('0') && digits[1..].starts_with(|c: char| c.is_ascii_digit()) {
        return Some(Err(format!("ion forbids a leading zero: {token}")));
    }

    let cleaned = digits.replace('_', "");
    if cleaned.bytes().all(|b| b.is_ascii_digit()) {
        return Some(match cleaned.parse::<i128>() {
            Ok(magnitude) => Ok(int_value(if negative { -magnitude } else { magnitude })),
            Err(_) => Err(format!("ion integer out of range: {token}")),
        });
    }
    // A decimal's `d`/`D` exponent, and the bare-`.` form Ion also calls a
    // decimal. Both become an `f64` here; see this module's doc for the one
    // divergence that costs.
    let normalised = cleaned.replacen(['d', 'D'], "e", 1);
    let value: f64 = normalised.parse().ok()?;
    Some(Ok(MjValue::from(if negative { -value } else { value })))
}

/// The narrowest integer that holds `magnitude`, so
/// `-9223372036854775808` and `12345678901234567890` both come back as
/// numbers rather than as errors — Kestra echoes both, and an `i64`-only
/// parse refused them.
fn int_value(magnitude: i128) -> MjValue {
    if let Ok(narrow) = i64::try_from(magnitude) {
        return MjValue::from(narrow);
    }
    if let Ok(wide) = u64::try_from(magnitude) {
        return MjValue::from(wide);
    }
    MjValue::from(magnitude)
}

/// Ion's operator characters, which are symbols **only inside an
/// s-expression**. `(+ 1 2)` is `["+",1,2]` in Kestra; the same `+` at the top
/// level is not a value.
fn is_ion_operator_char(c: char) -> bool {
    matches!(
        c,
        '!' | '#'
            | '%'
            | '&'
            | '*'
            | '+'
            | '-'
            | '.'
            | '/'
            | ';'
            | '<'
            | '='
            | '>'
            | '?'
            | '@'
            | '^'
            | '`'
            | '|'
            | '~'
    )
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
