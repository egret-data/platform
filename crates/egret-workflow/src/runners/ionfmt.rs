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
                // **Ion writes the UNSCALED digits and the scale**, where
                // `BigDecimal.toString()` writes a normalised mantissa. So
                // `1.5E+2` is `15d1` and not `1.5d2` — measured through
                // `'1.5E2' | number(type='BIGDECIMAL') | toIon`. The two agree
                // only when the mantissa has no fraction, which is exactly
                // where the existing entries sat.
                Some((mantissa, exponent)) => {
                    let fraction = mantissa.split_once('.').map_or(0, |(_, f)| f.len());
                    let unscaled: String = mantissa.chars().filter(|c| *c != '.').collect();
                    let exponent: i64 = exponent.parse().unwrap_or(0);
                    out.push_str(&unscaled);
                    out.push('d');
                    out.push_str(&(exponent - fraction as i64).to_string());
                }
                // **A scale-0 `BigDecimal` still needs a `.`**, because Ion
                // spells a decimal with a `.` or a `d` and an integer with
                // neither. Measured: `'1' | number(type='BIGDECIMAL') | toIon`
                // is `1.`, and this wrote `1` — a decimal that reads back as
                // an int. A `BigInteger` takes no dot, which is why the class
                // is consulted rather than the text alone.
                None => {
                    out.push_str(&text);
                    if super::filters::is_big_decimal(value) && !text.contains('.') {
                        out.push('.');
                    }
                }
            }
        }
        // Strings.
        _ => write_ion_string(out, &value.to_string()),
    }
}

/// One row of `egret.sql`'s `fetch: STORE` as an Ion struct,
/// `{column:value,…}` in column order, with no newline (W9a spec §9.2).
///
/// The value is the JSON a `TabularResult` carries, so the Ion type follows
/// the JSON type: an integer is written verbatim at any size, any other
/// number as an Ion float (what `ofIon` does with a `Double`), a string as a
/// string, an array as a list, an object as a struct. A DECIMAL, a date or a
/// timestamp that the connector carries as a string stays a string: Kestra's
/// file holds a decimal, an Ion timestamp or an annotated string there
/// (`w9a-storage.json` `ion_rows`), and the handler's test records each
/// divergence by type. A `null` is written, not dropped (`ofIon`'s inclusion
/// is `ALWAYS`, this module's doc).
pub(crate) fn write_ion_row(out: &mut String, columns: &[String], row: &[serde_json::Value]) {
    out.push('{');
    for (i, (name, cell)) in columns.iter().zip(row).enumerate() {
        if i > 0 {
            out.push(',');
        }
        write_symbol(out, name);
        out.push(':');
        write_ion_json(out, cell);
    }
    out.push('}');
}

/// One value an Ion READER yielded (`storage::ion`), written back as Ion
/// text, compact, with no newline -- what W9a Task 12's item tasks
/// (`FilterItems`, `DeduplicateItems`, `Split` of a binary Ion file) write
/// for each record they keep (spec §2, §9.3).
///
/// **Kestra writes those files as BINARY Ion** (`FileSerde.write`,
/// `w9a-storage.json` finding `store_binary_ion`); this writes text, so the
/// bytes never match and the VALUES do: every Ion type is written as itself
/// (a decimal keeps its scale, a timestamp its precision and offset, a blob
/// its bytes, a symbol stays a symbol, a typed null its type), with the
/// value's annotations in front of it, the layout this module's other
/// writers use (`{k:"a",v:1}`). Kestra's round trip through `FileSerde`
/// turns a symbol into a string and a timestamp into a `java.time` value
/// before writing; this keeps the value the file held (not measured beyond
/// the fixture's strings and ints). A struct field whose value is `null` is
/// written, as `ofIon`'s inclusion is `ALWAYS` (this module's doc).
///
/// The depth is the reader's: `storage::ion` never yields a value nested
/// deeper than its `MAX_DEPTH`, so this recursion is bounded by it.
pub(crate) fn write_ion_element(out: &mut String, element: &ion_rs::Element) {
    use base64::Engine as _;
    use ion_rs::Value as V;
    for annotation in element.annotations().iter() {
        write_symbol(out, annotation.text().unwrap_or(""));
        out.push_str("::");
    }
    match element.value() {
        V::Null(ion_rs::IonType::Null) => out.push_str("null"),
        V::Null(t) => {
            out.push_str("null.");
            out.push_str(&t.to_string());
        }
        V::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        V::Int(i) => out.push_str(&i.to_string()),
        V::Float(f) => write_ion_float(out, *f),
        // `ion-rs`'s Display of a decimal and of a timestamp is Ion text
        // (`12.340`, `1d3`, `12.`; `2026-01-02T03:04:05.123Z`).
        V::Decimal(d) => out.push_str(&d.to_string()),
        V::Timestamp(t) => out.push_str(&t.to_string()),
        V::Symbol(sym) => match sym.text() {
            Some(text) => write_symbol(out, text),
            // A symbol with unknown text: Ion's symbol zero.
            None => out.push_str("$0"),
        },
        V::String(text) => write_ion_string(out, text.text()),
        V::Clob(bytes) => {
            out.push_str("{{\"");
            for &b in bytes.as_ref() {
                match b {
                    b'"' => out.push_str("\\\""),
                    b'\\' => out.push_str("\\\\"),
                    0x20..=0x7e => out.push(b as char),
                    _ => out.push_str(&format!("\\x{b:02x}")),
                }
            }
            out.push_str("\"}}");
        }
        V::Blob(bytes) => {
            out.push_str("{{");
            out.push_str(&base64::engine::general_purpose::STANDARD.encode(bytes.as_ref()));
            out.push_str("}}");
        }
        V::List(seq) => {
            out.push('[');
            for (i, item) in seq.elements().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_ion_element(out, item);
            }
            out.push(']');
        }
        V::SExp(seq) => {
            out.push('(');
            for (i, item) in seq.elements().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                write_ion_element(out, item);
            }
            out.push(')');
        }
        V::Struct(st) => {
            out.push('{');
            for (i, (name, value)) in st.fields().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_symbol(out, name.text().unwrap_or(""));
                out.push(':');
                write_ion_element(out, value);
            }
            out.push('}');
        }
    }
}

/// One JSON value as Ion text, for [`write_ion_row`] (the KV store's
/// `storage::kv::value::encode` writes its own tree with [`write_ion_float`],
/// [`write_ion_string`] and [`write_symbol`]). Recursion is bounded by
/// the JSON's own depth, which `serde_json` caps when it parses.
pub(crate) fn write_ion_json(out: &mut String, value: &serde_json::Value) {
    use serde_json::Value as J;
    match value {
        J::Null => out.push_str("null"),
        J::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        J::Number(n) => {
            // `arbitrary_precision` is on in this crate, so the text is the
            // number as the connector spelled it: an integer of any size
            // stays exact, as an Ion int can be.
            let text = n.to_string();
            let digits = text.strip_prefix('-').unwrap_or(&text);
            if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
                out.push_str(&text);
            } else {
                match n.as_f64() {
                    Some(x) if x.is_finite() => write_ion_float(out, x),
                    // Out of a double's range: still a float, in Ion's
                    // spelling of the exponent.
                    _ => out.push_str(&text.replace('E', "e")),
                }
            }
        }
        J::String(s) => write_ion_string(out, s),
        J::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_ion_json(out, item);
            }
            out.push(']');
        }
        J::Object(map) => {
            out.push('{');
            for (i, (key, item)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_symbol(out, key);
                out.push(':');
                write_ion_json(out, item);
            }
            out.push('}');
        }
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
pub(crate) fn write_ion_float(out: &mut String, x: f64) {
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
pub(crate) fn write_ion_string(out: &mut String, s: &str) {
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
pub(crate) fn write_symbol(out: &mut String, s: &str) {
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
/// | `{{ aGVsbG8= }}` (a blob) | `"aGVsbG8="` — a `byte[]`, base64 on output | **refused** |
/// | `{{ "clob" }}` | `"Y2xvYg=="` — base64 of the bytes | **refused** |
/// | `2024-01-01T00:00:00Z`, `2024T` | `2024-01-01`, a `java.time.LocalDate` | **refused** |
///
/// None has a `minijinja::Value` that means the same thing, and `write_ion`
/// emits none of them, so nothing egret writes can produce one. Each is
/// refused **by name** — the same call `javafmt` makes for the three CLDR
/// pattern letters, and the only kind of divergence this crate's own rule
/// allows. A timestamp in particular would otherwise become a string, which
/// reads back as a different type with no error anywhere. All three are
/// corpus entries rather than prose.
///
/// **Annotations are not on this list, and getting them off it took two
/// corrections.** The first cut SKIPPED them, on the sentence quoted above;
/// the second REFUSED them, on a replacement sentence — "Kestra throws for one
/// inside a container" — that was wrong in exactly the same way. It does not:
/// `[a::1, 2] | length` is `2` and `{f:a::1} | className` is
/// `java.util.LinkedHashMap`. See [`IonReader::annotated`].
///
/// S-expressions were on that list and are not any more: Kestra reads `(1 2)`
/// as `[1,2]`, `(a b)` as `["a","b"]` and `(+ 1 2)` as `["+",1,2]`, so this
/// reader does too.
///
/// # `d` is a decimal and `e` is a float, and the difference is a Java class
///
/// Measured over fourteen literals, four ways each: `1d2` is `100` and
/// `java.math.BigDecimal`, `1E2` is `100.0` and `java.lang.Double`. Reading
/// the first family as an `f64` loses the scale — `1.50` came back `1.5` and
/// `1d2 | toIon` came back `100e0` — so a decimal is a
/// [`super::filters::big_decimal`].
///
/// An earlier version of this doc recorded that as a divergence and excused it
/// with "no round trip through this crate reaches it — only hand-written Ion
/// does". `{{ fromIon('1d2') | toIon }}` is a round trip through this crate
/// and it reached it.
///
/// # Bounded, because a struct nests
///
/// `[[[[…` is author-controlled input and this is a recursive descent over
/// it, on the render thread. The bound is [`jqsandbox::MAX_VALUE_DEPTH`], the
/// crate's one value-depth invariant, rather than a fifth number: *no value
/// anywhere in this crate is more than `MAX_VALUE_DEPTH` deep*.
pub(super) fn read_ion(text: &str, all_rows: bool) -> Result<Vec<MjValue>, String> {
    let mut reader = IonReader {
        rest: text,
        depth: 0,
        unterminated_comment: false,
    };
    let mut values = Vec::new();
    reader.skip_ignorable();
    // `$ion_1_0` is the version marker, not a value, and `$ion_1_1` is a
    // version this reader does not speak — Kestra throws
    // `UnsupportedIonVersionException` for that one.
    //
    // **The test is on the whole TOKEN, not on a prefix.** An earlier cut
    // stripped `$ion_1_` and refused anything after it, which traded four
    // correct answers for one: `$ion_1_`, `$ion_1_x`, `$ion_1_0abc` and
    // `$ion_1_0_x` are ordinary symbols and Kestra answers all four.
    if let Some(marker) = ion_version_marker(reader.rest) {
        if marker != "0" {
            return Err(format!("unsupported ion version marker: $ion_1_{marker}"));
        }
        reader.rest = &reader.rest["$ion_1_".len() + marker.len()..];
        reader.skip_ignorable();
    }
    while !reader.rest.is_empty() {
        values.push(reader.value()?);
        // **Stop at the first value unless the caller asked for all of them.**
        // `FileSerde.readAll(...).take(1).findFirst()` never looks at the
        // tail, so `fromIon('1 /*c')` is `1` and `fromIon('1 +5')` is `1` —
        // both were errors here the moment this reader grew refusals, because
        // it parsed the whole document eagerly and then took `rows[0]`.
        // `allRows=true` is `toList()` and DOES read the tail: measured, every
        // one of those documents throws there.
        if !all_rows {
            return Ok(values);
        }
        reader.skip_ignorable();
    }
    if reader.unterminated_comment {
        return Err("unterminated ion block comment".to_string());
    }
    Ok(values)
}

/// The digits of an Ion version marker at the head of `s`, if the token there
/// is exactly `$ion_1_<digits>`.
///
/// `None` for `$ion_1_`, `$ion_1_x`, `$ion_1_0abc` and `$ion_1_0_x`, every one
/// of which is an ordinary symbol that Kestra answers — measured.
fn ion_version_marker(s: &str) -> Option<&str> {
    let rest = s.strip_prefix("$ion_1_")?;
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    let digits = &rest[..end];
    if digits.is_empty() || starts_identifier_char(&rest[end..]) {
        return None;
    }
    Some(digits)
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
        // An annotated value (`sym::value`) becomes its own SOURCE TEXT. See
        // this module's doc: Jackson hands back an opaque `IonValue` whose
        // `toString()` is that text, so `[a::1, 2]` really is a two-element
        // list whose first element renders `a::1`.
        let save = self.rest;
        if let Ok(Some(_)) = self.symbol_like() {
            let annotated = self.rest.trim_start().starts_with("::");
            self.rest = save;
            if annotated {
                return self.annotated();
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

    /// An annotated value, as the text it was written with.
    ///
    /// Measured, and it is the correction to a correction: the first cut of
    /// this reader SKIPPED annotations on the strength of a "Kestra does"
    /// sentence written without running one, and answered `1` for `a::1`; the
    /// second REFUSED them, on a replacement sentence ("throws for one inside
    /// a container") that was wrong in the same way. What Kestra actually
    /// does, across fifteen probed forms:
    ///
    /// | source | Kestra renders | `className` |
    /// |---|---|---|
    /// | `a::1` | `a::1` | `com.amazon.ion.impl.lite.IonIntLite` |
    /// | `a::"s"` | `a::"s"` | `IonStringLite` |
    /// | `a::{b:1}` | `a::{b:1}` | `IonStructLite` |
    /// | `[a::1, 2]` | (throws) | `java.util.ArrayList`, `length` 2, `[1]` is 2 |
    ///
    /// So the annotated value is an opaque object, the CONTAINER is a normal
    /// list or map, and only stringifying a container that holds one fails —
    /// as a Jackson serialiser artifact whose message is not even stable
    /// across JVM runs (`Document nesting depth (1001) exceeds the maximum`
    /// once, `Direct self-reference leading to cycle` twice, on the same jar
    /// and the same corpus).
    ///
    /// Carrying it as text gets the container right, gets the bare render
    /// right, and leaves the divergences in `oracle/baseline.txt` rather than
    /// poisoning a whole document for one exotic element. The internal Amazon
    /// class name is deliberately **not** reproduced: `oracle/README.md`'s own
    /// rule excludes "an internal class name" from what a port copies.
    fn annotated(&mut self) -> Result<MjValue, String> {
        let start = self.rest;
        // Consume `sym::` runs, then the value they decorate, so the slice
        // this returns is exactly what was written.
        loop {
            let save = self.rest;
            let Ok(Some(_)) = self.symbol_like() else {
                self.rest = save;
                break;
            };
            self.skip_ignorable();
            match self.rest.strip_prefix("::") {
                Some(rest) => {
                    self.rest = rest;
                    self.skip_ignorable();
                }
                None => {
                    self.rest = save;
                    break;
                }
            }
        }
        // The decorated value is parsed and thrown away: it is what tells this
        // where the annotated text ENDS, and parsing it is also what keeps a
        // malformed one (`1 abc::`) an error rather than a symbol.
        let _ = self.value()?;
        let consumed = start.len() - self.rest.len();
        Ok(MjValue::from(start[..consumed].trim_end().to_string()))
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
    // **Every non-integer Ion number is an `f64` here, and Kestra splits them
    // in two.** An `e`/`E` exponent is a `java.lang.Double` and a `d`/`D` one
    // or a bare `.` is a `java.math.BigDecimal`, which is a first-class number
    // there: `1.5 * 2` is `3.00`, `1.5 > 1` is `true`, `(1.5 + 1) | className`
    // is `java.math.BigDecimal`.
    //
    // **It cannot be one here, and that is a property of minijinja rather than
    // a choice.** `filters::BigNumber` is an `Object`, and `value::ops::coerce`
    // matches on `ValueRepr` alone with no object arm, while `Ord for Value`
    // compares `kind()` FIRST and `ValueKind::Plain` sorts above
    // `ValueKind::Number`. So routing Ion decimals through `BigNumber` — which
    // an earlier cut did, to keep the scale — made every `+ - * / %` an error
    // and made **every `>`/`>=` unconditionally true and every `<`/`<=`
    // unconditionally false**, silently. `Object::custom_cmp` does not help:
    // it is consulted only when both sides are objects of the same type.
    //
    // Ion is the format Kestra task outputs are written in, so
    // `{{ fromIon(...) }}` followed by arithmetic is the ordinary case. A
    // wrong SPELLING is recorded; a wrong COMPARISON is not acceptable, so the
    // scale is what gives way. Five entries record what that costs, and
    // `filter-number-bigdecimal-is-not-a-number-here` records the same hole on
    // the `number(type='BIGDECIMAL')` path, which still has it.
    let signed = if negative {
        format!("-{cleaned}")
    } else {
        cleaned
    };
    // A decimal's exponent letter is the only difference from a float's.
    let normalised = signed.replacen(['d', 'D'], "e", 1);
    let value: f64 = normalised.parse().ok()?;
    Some(Ok(MjValue::from(value)))
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

    fn row(columns: &[&str], cells: Vec<serde_json::Value>) -> String {
        let columns: Vec<String> = columns.iter().map(|c| c.to_string()).collect();
        let mut out = String::new();
        write_ion_row(&mut out, &columns, &cells);
        out
    }

    /// `egret.sql` STORE's row writer: a struct per row in column order,
    /// field names quoted as `write_symbol` does, an integer verbatim at any
    /// size, any other number as an Ion FLOAT (Kestra writes a `Double` that
    /// way), nested JSON as Ion containers, and `null` written, not dropped.
    #[test]
    fn a_row_is_one_struct_in_column_order() {
        use serde_json::json;
        assert_eq!(
            row(&["b", "a"], vec![json!(1), json!("x")]),
            "{b:1,a:\"x\"}",
            "column order, not key order"
        );
        assert_eq!(
            row(
                &["null", "a b", "$1"],
                vec![json!(true), json!(false), json!(null)]
            ),
            "{'null':true,'a b':false,'$1':null}"
        );
        let big: serde_json::Value =
            serde_json::from_str("123456789012345678901234567890").unwrap();
        let neg: serde_json::Value = serde_json::from_str("-9007199254740993").unwrap();
        assert_eq!(
            row(
                &["i", "j", "f", "g"],
                vec![big, neg, json!(1.5), json!(1e100)]
            ),
            "{i:123456789012345678901234567890,j:-9007199254740993,f:1.5e0,g:1.0e100}"
        );
        assert_eq!(
            row(
                &["l", "m"],
                vec![json!([1, null, "a"]), json!({"k": {"true": 2}})]
            ),
            "{l:[1,null,\"a\"],m:{k:{'true':2}}}"
        );
        assert_eq!(row(&["s"], vec![json!("a\nb\"c")]), "{s:\"a\\nb\\\"c\"}");
        assert_eq!(row(&[], vec![]), "{}");
    }
}
