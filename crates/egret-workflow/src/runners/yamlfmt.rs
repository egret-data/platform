// kestra: core/src/main/java/io/kestra/core/runners/pebble/filters/YamlFilter.java
// jackson: com.fasterxml.jackson.dataformat.yaml.YAMLGenerator (2.22.1)
// jackson: com.fasterxml.jackson.dataformat.yaml.util.StringQuotingChecker (2.22.1)
// snakeyaml: org.yaml.snakeyaml.emitter.Emitter (2.5)

//! The `yaml` filter's emitter: Jackson's `YAMLGenerator` sitting on
//! SnakeYAML's `Emitter`, with the seven `YAMLGenerator.Feature`s
//! `YamlFilter`'s static initialiser sets.
//!
//! Those seven, read off `javap -p -c YamlFilter.class`, are the whole
//! configuration and each of them is visible in the output:
//!
//! | feature | value | what it does here |
//! |---|---|---|
//! | `MINIMIZE_QUOTES` | on | plain scalars wherever YAML allows one |
//! | `WRITE_DOC_START_MARKER` | off | no leading `---` … except one case, below |
//! | `USE_NATIVE_TYPE_ID` | off | no `!!` tags |
//! | `SPLIT_LINES` | off | a 200-character scalar stays on one line |
//! | `INDENT_ARRAYS` | on | the `-` sits at *parent + 1*, not at the parent |
//! | `USE_PLATFORM_LINE_BREAKS` | on | `\n` here, `\r\n` on Windows |
//! | `ALWAYS_QUOTE_NUMBERS_AS_STRINGS` | off | `{{ '42' \| yaml }}` is `42` |
//!
//! The last one is worth reading twice: **a string that looks like a number
//! comes out unquoted and reads back as a number.** `'42' | yaml` is `42\n`.
//! That is upstream's configuration, not a defect of this port.
//!
//! # Two layers decide the quoting, and they disagree
//!
//! Jackson picks a *requested* style per scalar
//! (`YAMLGenerator.writeString`); SnakeYAML then **downgrades** it when YAML
//! cannot express the value that way. Everything Jackson forces comes out
//! double-quoted; everything SnakeYAML downgrades comes out single-quoted or
//! double. That is why `'yes'` is `"yes"` and `'&a'` is `'&a'` — same filter,
//! same input shape, two different quote characters, from two different
//! layers. [`scalar_style`] is the first layer and [`plain_allowed`] /
//! [`single_allowed`] are the second.
//!
//! # What this does not reproduce
//!
//! `\u{85}` (NEL), `\u{2028}` and `\u{2029}` are YAML 1.1 line breaks, and
//! SnakeYAML folds a plain scalar across them: `{{ 'a\u{85}b' | yaml }}` is
//! `'a\u{85}  b'` in Kestra — the emitter re-indents after what it read as a
//! break, and two spaces appear that were not in the input. egret writes
//! `"a\u{85}b"`. Recorded as a divergence rather than ported: the two spaces
//! are a re-indentation artefact of a line-break the input did not intend,
//! and no round trip recovers the original string from Kestra's output
//! either. See `oracle/README.md`.

use minijinja::value::{Value as MjValue, ValueKind};

use super::env::write_java_double;

/// A map key is written as a **simple** key only up to this length; at it and
/// past it SnakeYAML emits the explicit `? key\n: value` form.
///
/// Measured at 127: `'x' * 127` is a simple key and `'x' * 128` is not. The
/// constant in SnakeYAML's `Emitter` is 128 and its comparison is `>=`, which
/// is exactly the off-by-one a reader of that source would get wrong — an
/// earlier cut of this file said 128 and the sweep caught it on the one probe
/// that sits on the boundary.
///
/// **The unit is a UTF-16 code unit**, because the constant is compared
/// against `String.length()`. An astral character counts as two: 64 emoji is
/// a 64-character, 128-unit key, and it is NOT simple. Every ASCII probe
/// agrees under either reading, which is why
/// `filter-yaml-key-at-the-simple-limit` could not tell them apart and
/// `filter-yaml-astral-key-counts-utf16-units` can.
const MAX_SIMPLE_KEY_LENGTH: usize = 127;

/// Jackson's `_writeScalar` indent, i.e. `DumperOptions.setIndent`'s default.
const INDENT: usize = 2;

/// Renders `value` the way the `yaml` filter does, trailing newline included.
pub(super) fn to_yaml(value: &MjValue) -> String {
    let mut out = String::new();
    match container(value) {
        Some(Container::Map(entries)) if !entries.is_empty() => write_map(&mut out, &entries, 0),
        Some(Container::Seq(items)) if !items.is_empty() => write_seq(&mut out, &items, 0),
        _ => {
            // A lone empty string at the root is the one place
            // `WRITE_DOC_START_MARKER=false` does not win: SnakeYAML emits the
            // marker because a document whose whole content is an empty
            // quoted scalar is otherwise unreadable. Measured, not reasoned
            // about -- `{{ '' | yaml }}` is `--- ""\n`.
            if value.kind() == ValueKind::String && value.as_str() == Some("") {
                out.push_str("--- \"\"");
            } else {
                write_scalar(&mut out, value, INDENT);
            }
            out.push('\n');
        }
    }
    out
}

/// The two container shapes, collected once so emptiness and iteration order
/// are decided in one place.
enum Container {
    Map(Vec<(String, MjValue)>),
    Seq(Vec<MjValue>),
}

fn container(value: &MjValue) -> Option<Container> {
    match value.kind() {
        ValueKind::Map => {
            let keys = value.try_iter().ok()?;
            Some(Container::Map(
                keys.map(|key| {
                    let item = value.get_item(&key).unwrap_or_default();
                    (key.to_string(), item)
                })
                .collect(),
            ))
        }
        ValueKind::Seq | ValueKind::Iterable => {
            Some(Container::Seq(value.try_iter().ok()?.collect()))
        }
        _ => None,
    }
}

fn write_map(out: &mut String, entries: &[(String, MjValue)], indent: usize) {
    for (key, value) in entries {
        let rendered = key_repr(key);
        if is_simple_key(key) {
            out.push_str(&" ".repeat(indent));
            out.push_str(&rendered);
        } else {
            // `? key` on its own line, then `: value` -- the explicit-key
            // form, which SnakeYAML falls back to for an empty key, a key
            // with a line break in it, and a key past 128 characters.
            out.push_str(&" ".repeat(indent));
            out.push_str("? ");
            out.push_str(&rendered);
            out.push('\n');
            out.push_str(&" ".repeat(indent));
        }
        out.push(':');
        write_value_after_key(out, value, indent);
    }
}

/// The part after `key:` — either ` scalar` on the same line, or a newline
/// and an indented child.
fn write_value_after_key(out: &mut String, value: &MjValue, indent: usize) {
    match container(value) {
        Some(Container::Map(entries)) if !entries.is_empty() => {
            out.push('\n');
            write_map(out, &entries, indent + INDENT);
        }
        // A nested sequence keeps the MAP's indent, and its `-` then lands one
        // column further in. That is `INDENT_ARRAYS`: `{"a":[1]}` is
        // `a:\n - 1\n`, with the dash at column 1 and not at column 2.
        Some(Container::Seq(items)) if !items.is_empty() => {
            out.push('\n');
            write_seq(out, &items, indent);
        }
        _ => {
            out.push(' ');
            write_scalar(out, value, indent + INDENT);
            out.push('\n');
        }
    }
}

fn write_seq(out: &mut String, items: &[MjValue], indent: usize) {
    for item in items {
        out.push_str(&" ".repeat(indent + 1));
        out.push('-');
        match container(item) {
            Some(Container::Map(entries)) if !entries.is_empty() => {
                out.push('\n');
                write_map(out, &entries, indent + INDENT);
            }
            Some(Container::Seq(nested)) if !nested.is_empty() => {
                out.push('\n');
                write_seq(out, &nested, indent + INDENT);
            }
            _ => {
                out.push(' ');
                write_scalar(out, item, indent + INDENT);
                out.push('\n');
            }
        }
    }
}

/// One scalar, in the style the two layers between them choose.
///
/// `block_indent` is where a literal block scalar's body goes; it is the only
/// reason this needs to know where it is on the page.
fn write_scalar(out: &mut String, value: &MjValue, block_indent: usize) {
    match value.kind() {
        ValueKind::Undefined | ValueKind::None => out.push_str("null"),
        ValueKind::Bool => out.push_str(if value.is_true() { "true" } else { "false" }),
        ValueKind::Number => {
            if value.is_integer() {
                out.push_str(&value.to_string());
            } else {
                match f64::try_from(value.clone()) {
                    Ok(f) => write_java_double(out, f),
                    Err(_) => out.push_str(&value.to_string()),
                }
            }
        }
        ValueKind::Map => out.push_str("{}"),
        ValueKind::Seq | ValueKind::Iterable => out.push_str("[]"),
        // A `BigDecimal`/`BigInteger` is a number and is written unquoted, in
        // its `toString` spelling -- `1E+3`, not the `1000` a bare `{{ x }}`
        // renders. See `super::filters::big_number_text`.
        _ if super::filters::big_number_text(value).is_some() => {
            out.push_str(&super::filters::big_number_text(value).expect("checked"));
        }
        _ => write_string_scalar(out, &value.to_string(), block_indent),
    }
}

/// The style Jackson requests and SnakeYAML may downgrade.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Style {
    Plain,
    Single,
    Double,
    Literal,
}

fn write_string_scalar(out: &mut String, s: &str, block_indent: usize) {
    match scalar_style(s) {
        Style::Plain => out.push_str(s),
        Style::Single => write_single_quoted(out, s),
        Style::Double => write_double_quoted(out, s),
        Style::Literal => write_literal_block(out, s, block_indent),
    }
}

/// `YAMLGenerator.writeString` followed by SnakeYAML's downgrade.
///
/// Jackson's half, read off the bytecode:
///
/// ```text
/// if (text.isEmpty())                   -> DOUBLE
/// if (text.indexOf('\n') >= 0)          -> LITERAL
/// if (quotingChecker.needToQuoteValue)  -> DOUBLE
/// else                                  -> PLAIN
/// ```
///
/// SnakeYAML's half then answers whether YAML can actually write it that way.
fn scalar_style(s: &str) -> Style {
    if s.is_empty() {
        return Style::Double;
    }
    if s.contains('\n') {
        return if literal_allowed(s) {
            Style::Literal
        } else {
            Style::Double
        };
    }
    if need_to_quote_value(s) {
        return Style::Double;
    }
    if plain_allowed(s) {
        Style::Plain
    } else if single_allowed(s) {
        Style::Single
    } else {
        Style::Double
    }
}

/// `StringQuotingChecker.Default.needToQuoteValue` =
/// `isReservedKeyword || valueHasQuotableChar`, both decompiled.
///
/// Note what is **not** in it: `looksLikeYAMLNumber` is defined on the same
/// class and is consulted for *names* only. That is the whole reason
/// `{{ '42' | yaml }}` is `42` and `{{ {'42': 1} | yaml }}` is `"42": 1`.
fn need_to_quote_value(s: &str) -> bool {
    is_reserved_keyword(s) || value_has_quotable_char(s)
}

/// `needToQuoteName` — the same reserved-word test plus `looksLikeYAMLNumber`,
/// and **without** `valueHasQuotableChar`.
///
/// Measured both directions: `{'a,b': 1}` is `a,b: 1` (a comma is quotable in
/// a value and not in a name) and `{'-a': 1}` is `"-a": 1` (a leading `-`
/// makes a name look like a number and does nothing to a value).
fn need_to_quote_name(s: &str) -> bool {
    is_reserved_keyword(s) || looks_like_yaml_number(s)
}

/// The 24 words plus `~`, plus the empty string, from
/// `StringQuotingChecker`'s `RESERVED_KEYWORDS` set and its `_isReservedKeyword`
/// switch (which only consults the set when the first character is one of
/// `FNOTYfnoty`, and answers `true` outright for `~`).
fn is_reserved_keyword(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    match s.as_bytes()[0] {
        b'~' => true,
        b'F' | b'N' | b'O' | b'T' | b'Y' | b'f' | b'n' | b'o' | b't' | b'y' => matches!(
            s,
            "false"
                | "False"
                | "FALSE"
                | "n"
                | "N"
                | "no"
                | "No"
                | "NO"
                | "null"
                | "Null"
                | "NULL"
                | "on"
                | "On"
                | "ON"
                | "off"
                | "Off"
                | "OFF"
                | "true"
                | "True"
                | "TRUE"
                | "y"
                | "Y"
                | "yes"
                | "Yes"
                | "YES"
        ),
        _ => false,
    }
}

/// `_looksLikeYAMLNumber` is a switch on the **first character only**:
/// `+`, `-`, `.` and `0`-`9` answer true, everything else false. It never
/// looks at the rest of the string, which is why `2024-01-15` and `.inf` are
/// both quoted as names.
fn looks_like_yaml_number(s: &str) -> bool {
    matches!(s.as_bytes().first(), Some(b'+' | b'-' | b'.' | b'0'..=b'9'))
}

/// `valueHasQuotableChar`: any of `,[]{}`; a `#` at the start or after a
/// blank; a `:` at the end or before a blank.
fn value_has_quotable_char(s: &str) -> bool {
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b',' | b'[' | b']' | b'{' | b'}' => return true,
            b'#' => {
                if i == 0 || is_blank(bytes[i - 1]) {
                    return true;
                }
            }
            b':' if i == bytes.len() - 1 || is_blank(bytes[i + 1]) => return true,
            _ => {}
        }
    }
    false
}

/// A `:` followed by a blank, anywhere.
fn value_has_colon_space(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes
        .iter()
        .enumerate()
        .any(|(i, &b)| b == b':' && i + 1 < bytes.len() && is_blank(bytes[i + 1]))
}

fn is_blank(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

/// Can SnakeYAML write this as a plain (unquoted) scalar?
///
/// Every clause was measured; the octal one is the surprise. `007`, `01`,
/// `00`, `0789` and `010` are all single-quoted while `0`, `0x0`, `0b1`,
/// `0.7`, `0e7`, `08.5`, `+007` and `-007` are plain — so the rule is
/// `^0[0-9_]+$` and nothing narrower. `0789` is not octal and is quoted
/// anyway.
fn plain_allowed(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    // A non-printable character is not plain and not single-quotable either;
    // it goes straight to double quotes.
    if s.chars().any(is_non_printable) {
        return false;
    }
    let bytes = s.as_bytes();
    if is_blank(bytes[0]) || is_blank(bytes[bytes.len() - 1]) {
        return false;
    }
    // The YAML indicator characters, as a leading character.
    if matches!(
        bytes[0],
        b'#' | b','
            | b'['
            | b']'
            | b'{'
            | b'}'
            | b'&'
            | b'*'
            | b'!'
            | b'|'
            | b'>'
            | b'\''
            | b'"'
            | b'%'
            | b'@'
            | b'`'
    ) {
        return false;
    }
    // `-`, `?` and `:` are indicators only when a blank (or the end) follows,
    // which is why `-a`, `?a` and `:a` are plain and `- a`, `? a` and `-` are
    // not.
    if matches!(bytes[0], b'-' | b'?' | b':') && (bytes.len() == 1 || is_blank(bytes[1])) {
        return false;
    }
    if s.starts_with("---") || s.starts_with("...") {
        return false;
    }
    // YAML's own plain-scalar restrictions, which are NOT the same list as
    // Jackson's `valueHasQuotableChar` even though they overlap: a `: ` or a
    // trailing `:` or a ` #` cannot appear in a plain scalar at all. On the
    // value side Jackson gets there first and forces double quotes; on the
    // NAME side nothing does, so `{'a: b': 1}` is `'a: b': 1` -- single
    // quotes, from this clause. Measured.
    if s.ends_with(':') || value_has_colon_space(s) || s.contains(" #") || s.contains("\t#") {
        return false;
    }
    if bytes[0] == b'0'
        && bytes.len() > 1
        && bytes[1..].iter().all(|b| b.is_ascii_digit() || *b == b'_')
    {
        return false;
    }
    true
}

/// Single quotes can hold anything printable — the only escape is `''` for a
/// quote — so this is exactly "no non-printable characters".
fn single_allowed(s: &str) -> bool {
    !s.chars().any(is_non_printable)
}

/// A literal block scalar holds raw lines, so it cannot hold a character YAML
/// would have to escape, and it cannot hold a line whose last character is a
/// blank (the trailing blank would be unrecoverable). Measured: `'x \ny'`,
/// `'x\n '` and `' \nx'` are all double-quoted for exactly that reason, and
/// `'a\r\nb'` is double-quoted for the first.
fn literal_allowed(s: &str) -> bool {
    if s.chars().any(|c| c != '\n' && is_non_printable(c)) {
        return false;
    }
    s.split('\n')
        .all(|line| !line.ends_with(' ') && !line.ends_with('\t'))
}

/// C0 (tab included — YAML's `\t` is not printable in a plain scalar), DEL,
/// and C1. `\u{a0}` is printable and stays plain, which is measured; the C1
/// block one code point below it is not.
///
/// `\n` reaches here only through [`literal_allowed`], which asks about it
/// separately.
fn is_non_printable(c: char) -> bool {
    let n = c as u32;
    n < 0x20 || (0x7f..=0x9f).contains(&n)
}

fn write_single_quoted(out: &mut String, s: &str) {
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
}

fn write_double_quoted(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0b}' => out.push_str("\\v"),
            '\u{0c}' => out.push_str("\\f"),
            '\u{08}' => out.push_str("\\b"),
            '\u{07}' => out.push_str("\\a"),
            '\0' => out.push_str("\\0"),
            '\u{1b}' => out.push_str("\\e"),
            // YAML's four NAMED escapes beyond the C-like ones.
            //
            // **`\L` and `\P` are unreachable today**, and that matches
            // Java: the style selector never picks a double-quoted scalar for
            // a string whose only unusual character is U+2028 or U+2029 —
            // SnakeYAML folds a PLAIN scalar across them instead, which is
            // the divergence `filter-yaml-next-line-character-folds` records.
            // They are here so that a value which reaches this writer by some
            // other route is written correctly rather than raw.
            //
            // **The read-back hazard is still there**, on both engines: a raw
            // U+0085, U+2028 or U+2029 in a scalar is a LINE BREAK to a YAML
            // 1.1 reader, so Kestra's own plain-scalar output does not round
            // trip either. Escaping is what avoids it, and Kestra does not
            // escape.
            '\u{85}' => out.push_str("\\N"),
            '\u{a0}' => out.push_str("\\_"),
            '\u{2028}' => out.push_str("\\L"),
            '\u{2029}' => out.push_str("\\P"),
            c if is_non_printable(c) => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// `|` with its two hints.
///
/// - the **indentation indicator** (`|2`) appears when the first character is
///   a space or a line break, because a reader cannot otherwise tell the
///   block's indent from the content's;
/// - the **chomping indicator** is `-` when the value does not end in a
///   newline, `+` when stripping one still leaves a newline (or leaves
///   nothing), and absent otherwise. `'\n'` alone is `|2+`, which is the case
///   that makes "ends with two newlines" the wrong rule.
fn write_literal_block(out: &mut String, s: &str, indent: usize) {
    out.push('|');
    if s.starts_with(' ') || s.starts_with('\n') {
        // **The indicator is RELATIVE, and SnakeYAML always writes 2.** It is
        // how much further the body is indented than the block's parent, not
        // where the body sits on the page. Writing the absolute column was
        // wrong everywhere except at the root, where the two coincide -- and
        // past depth 4 it emitted `|10-`, which is not valid YAML at all,
        // because the indicator is a single digit 1-9.
        //
        // `filter-yaml-literal-block-indent-indicator` used a ROOT scalar and
        // so could not tell the two rules apart;
        // `filter-yaml-literal-block-indicator-is-relative` is the entry that
        // can.
        out.push('2');
    }
    match s.strip_suffix('\n') {
        None => out.push('-'),
        Some(rest) if rest.is_empty() || rest.ends_with('\n') => out.push('+'),
        Some(_) => {}
    }

    let mut lines: Vec<&str> = s.split('\n').collect();
    // `split` leaves a trailing empty element for the final newline; that
    // element is the newline, not a line of its own.
    if lines.last() == Some(&"") {
        lines.pop();
    }
    let pad = " ".repeat(indent);
    for line in lines {
        out.push('\n');
        // An empty line is written empty -- no trailing whitespace.
        if !line.is_empty() {
            out.push_str(&pad);
            out.push_str(line);
        }
    }
}

/// A map key: Jackson's name-side quoting rules, then SnakeYAML's downgrade.
fn key_repr(key: &str) -> String {
    let mut out = String::new();
    // An empty key is `isReservedKeyword`'s own first clause, so the two
    // arms are one condition rather than two that happen to agree.
    let style = if need_to_quote_name(key) {
        Style::Double
    } else if plain_allowed(key) && !key.contains('\n') {
        Style::Plain
    } else if single_allowed(key) && !key.contains('\n') {
        Style::Single
    } else {
        Style::Double
    };
    match style {
        Style::Plain => out.push_str(key),
        Style::Single => write_single_quoted(&mut out, key),
        // A key is never written as a block scalar: `_writeFieldName` has no
        // `LITERAL` branch at all, so `{'a\nb': 1}` is `? "a\nb"`, measured.
        Style::Double | Style::Literal => write_double_quoted(&mut out, key),
    }
    out
}

/// Can this key go on the same line as its value?
fn is_simple_key(key: &str) -> bool {
    !key.is_empty() && key.encode_utf16().count() <= MAX_SIMPLE_KEY_LENGTH && !key.contains('\n')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runners::env::RenderContext;

    fn yaml_of(json: &str) -> String {
        let cx = RenderContext::from_json_str(&format!("{{\"v\": {json}}}")).expect("json");
        let value = cx.bindings()[0].1.clone();
        to_yaml(&value)
    }

    fn yaml_str(s: &str) -> String {
        to_yaml(&MjValue::from(s))
    }

    /// Every one of these is a golden measured through the Java harness, and
    /// they are the cases that separate the two quoting layers: `yes` and
    /// `42` are both "number-or-keyword shaped" and only one is quoted;
    /// `&a` and `,a` both start with an indicator and get different quotes.
    #[test]
    fn the_two_quoting_layers_pick_different_quote_characters() {
        assert_eq!(yaml_str("yes"), "\"yes\"\n");
        assert_eq!(yaml_str("42"), "42\n");
        assert_eq!(yaml_str("&a"), "'&a'\n");
        assert_eq!(yaml_str(",a"), "\",a\"\n");
        assert_eq!(yaml_str("a: b"), "\"a: b\"\n");
        assert_eq!(yaml_str("a:b"), "a:b\n");
        assert_eq!(yaml_str("a #b"), "\"a #b\"\n");
        assert_eq!(yaml_str("a#b"), "a#b\n");
    }

    /// `^0[0-9_]+$` and nothing narrower -- `0789` is not octal and is quoted
    /// anyway, `08.5` has a dot and is not.
    #[test]
    fn a_leading_zero_run_is_not_plain() {
        assert_eq!(yaml_str("007"), "'007'\n");
        assert_eq!(yaml_str("0789"), "'0789'\n");
        assert_eq!(yaml_str("0_1"), "'0_1'\n");
        assert_eq!(yaml_str("0"), "0\n");
        assert_eq!(yaml_str("0x0"), "0x0\n");
        assert_eq!(yaml_str("08.5"), "08.5\n");
        assert_eq!(yaml_str("+007"), "+007\n");
    }

    #[test]
    fn literal_blocks_carry_their_indent_and_chomping_hints() {
        assert_eq!(yaml_str("a\nb"), "|-\n  a\n  b\n");
        assert_eq!(yaml_str("a\n"), "|\n  a\n");
        assert_eq!(yaml_str("a\n\nb"), "|-\n  a\n\n  b\n");
        assert_eq!(yaml_str("\na"), "|2-\n\n  a\n");
        assert_eq!(
            yaml_str("  indented\n  more"),
            "|2-\n    indented\n    more\n"
        );
        assert_eq!(yaml_str("\n"), "|2+\n\n");
        assert_eq!(yaml_str("x\ny\n\n"), "|+\n  x\n  y\n\n");
        // A line ending in a blank cannot be a block scalar at all.
        assert_eq!(yaml_str("x \ny"), "\"x \\ny\"\n");
        assert_eq!(yaml_str("a\r\nb"), "\"a\\r\\nb\"\n");
    }

    /// `INDENT_ARRAYS` puts the dash one column in from its parent, and a
    /// sequence under a map key keeps the MAP's indent rather than gaining
    /// one. Both are what makes this layout unlike every other YAML writer's.
    #[test]
    fn nesting_follows_indent_arrays() {
        assert_eq!(yaml_of("{\"a\": [1, 2]}"), "a:\n - 1\n - 2\n");
        assert_eq!(yaml_of("[[[1]]]"), " -\n   -\n     - 1\n");
        assert_eq!(yaml_of("[{\"a\": {\"b\": 1}}]"), " -\n  a:\n    b: 1\n");
        assert_eq!(
            yaml_of("{\"a\": [{\"b\": [{\"c\": 1}]}]}"),
            "a:\n -\n  b:\n   -\n    c: 1\n"
        );
        assert_eq!(yaml_of("{\"a\": {}}"), "a: {}\n");
        assert_eq!(yaml_of("[{}]"), " - {}\n");
    }

    /// The name side quotes what the value side does not, and the other way
    /// round.
    #[test]
    fn names_and_values_do_not_share_a_quoting_rule() {
        assert_eq!(yaml_of("{\"42\": 1}"), "\"42\": 1\n");
        assert_eq!(yaml_of("{\"a,b\": 1}"), "a,b: 1\n");
        assert_eq!(yaml_of("{\"-a\": 1}"), "\"-a\": 1\n");
        assert_eq!(yaml_of("{\"2024-01-15\": 1}"), "\"2024-01-15\": 1\n");
        assert_eq!(yaml_of("{\"a: b\": 1}"), "'a: b': 1\n");
    }

    #[test]
    fn a_key_that_cannot_be_simple_gets_the_explicit_form() {
        assert_eq!(yaml_of("{\"\": 1}"), "? \"\"\n: 1\n");
        assert_eq!(yaml_of("{\"a\\nb\": 1}"), "? \"a\\nb\"\n: 1\n");
        let long = "k".repeat(128);
        assert_eq!(
            yaml_of(&format!("{{\"{long}\": 1}}")),
            format!("? {long}\n: 1\n")
        );
        let at_limit = "k".repeat(127);
        assert_eq!(
            yaml_of(&format!("{{\"{at_limit}\": 1}}")),
            format!("{at_limit}: 1\n")
        );
    }

    /// The root empty string is the only place the document-start marker
    /// comes back with `WRITE_DOC_START_MARKER` off.
    #[test]
    fn a_root_empty_string_writes_the_document_marker() {
        assert_eq!(yaml_str(""), "--- \"\"\n");
        assert_eq!(yaml_of("{\"a\": \"\"}"), "a: \"\"\n");
    }

    /// The `yaml` filter's mapper is built in `YamlFilter`'s own static
    /// initialiser and never gets `JacksonMapper.configure`'s `NON_NULL`, so
    /// a null map value survives here and does not survive `toJson`.
    #[test]
    fn yaml_keeps_a_null_map_value() {
        assert_eq!(yaml_of("{\"a\": null, \"b\": 1}"), "a: null\nb: 1\n");
    }
}
