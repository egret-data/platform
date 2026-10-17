//! Dialect-aware SQL literal escaping — the only place in the workspace where a
//! string value becomes a SQL literal — and, beside it, identifier quoting
//! ([`quote_ident`]).
//!
//! Trino does not process backslash escapes inside standard string literals, so
//! doubling `'` is sufficient and doubling `\` would corrupt the value. StarRocks
//! is MySQL-dialect and, unless `NO_BACKSLASH_ESCAPES` is set, treats `\` as an
//! escape character — so a value containing `\'` escapes the quote and breaks out
//! of a literal that only doubled quotes.
//!
//! Quotes are escaped as `''` rather than `\'` on purpose: that keeps the two
//! substitution passes order-independent, because the quote pass emits no
//! backslash and the backslash pass emits no quote.
//!
//! This lives in the contract crate, not in `egret-query`, so that `egret-query`
//! (SQL Lab) and `egret-bi` (chart/dashboard filters) share one implementation.
//! Two copies of this rule are what allowed them to drift apart.

use crate::Engine;

/// Quote and escape `value` as a SQL string literal for `engine`.
pub fn quote_string(engine: Engine, value: &str) -> String {
    match engine {
        Engine::Trino => format!("'{}'", value.replace('\'', "''")),
        Engine::StarRocks => {
            let escaped = value.replace('\\', "\\\\").replace('\'', "''");
            format!("'{escaped}'")
        }
    }
}

/// Quote `ident` as a delimited identifier for `engine`: Trino `"…"` with an
/// embedded `"` doubled, StarRocks `` `…` `` with an embedded backtick doubled.
///
/// Neither delimited form processes backslash escapes (both grammars define the
/// quoted identifier as "anything but the delimiter, or the delimiter
/// doubled"), so a backslash, a `.` or the other engine's delimiter is an
/// ordinary character inside it. That is the difference from [`quote_string`],
/// where a StarRocks backslash IS an escape and must be doubled (#504).
pub fn quote_ident(engine: Engine, ident: &str) -> String {
    match engine {
        Engine::Trino => format!("\"{}\"", ident.replace('"', "\"\"")),
        Engine::StarRocks => format!("`{}`", ident.replace('`', "``")),
    }
}

#[cfg(test)]
mod ident_tests {
    use super::quote_ident;
    use crate::Engine;

    #[test]
    fn each_engine_doubles_only_its_own_delimiter() {
        let name = r#"a"b`c.d\e"#;
        assert_eq!(quote_ident(Engine::Trino, name), r#""a""b`c.d\e""#);
        assert_eq!(quote_ident(Engine::StarRocks, name), r#"`a"b``c.d\e`"#);
    }

    /// A trailing backslash would escape a StarRocks string literal's closing
    /// quote (#504); inside a delimited identifier it is just a character, so
    /// it is never doubled — doubling it would name a different object.
    #[test]
    fn a_trailing_backslash_stays_single() {
        assert_eq!(quote_ident(Engine::Trino, r"s\"), r#""s\""#);
        assert_eq!(quote_ident(Engine::StarRocks, r"s\"), r"`s\`");
    }

    #[test]
    fn case_and_a_dot_stay_inside_one_identifier() {
        assert_eq!(quote_ident(Engine::Trino, "Sales.EU"), r#""Sales.EU""#);
        assert_eq!(quote_ident(Engine::StarRocks, "Sales.EU"), "`Sales.EU`");
    }

    #[test]
    fn a_name_made_of_delimiters_cannot_close_the_identifier() {
        assert_eq!(quote_ident(Engine::Trino, "\""), "\"\"\"\"");
        assert_eq!(quote_ident(Engine::StarRocks, "`"), "````");
    }
}

#[cfg(test)]
mod tests {
    use super::quote_string;
    use crate::Engine;

    /// An empty value still renders as a well-formed empty literal on both
    /// dialects — not as a bare pair of quotes that a caller could mistake for
    /// a missing argument.
    #[test]
    fn empty_value_renders_empty_literal() {
        assert_eq!(quote_string(Engine::Trino, ""), "''");
        assert_eq!(quote_string(Engine::StarRocks, ""), "''");
    }

    /// A lone trailing backslash is the minimal breakout on StarRocks: left
    /// alone it escapes the closing quote and the literal runs on into the rest
    /// of the statement. It must be doubled. On Trino a backslash is an
    /// ordinary character and must pass through untouched.
    #[test]
    fn lone_trailing_backslash() {
        assert_eq!(quote_string(Engine::Trino, r"C:\"), r"'C:\'");
        assert_eq!(quote_string(Engine::StarRocks, r"C:\"), r"'C:\\'");
    }

    /// A value that is nothing but backslashes: every one is doubled on
    /// StarRocks (3 in, 6 out) and none on Trino.
    #[test]
    fn only_backslashes() {
        assert_eq!(quote_string(Engine::Trino, r"\\\"), r"'\\\'");
        assert_eq!(quote_string(Engine::StarRocks, r"\\\"), r"'\\\\\\'");
    }

    /// Escaping is byte-substitution over `char`s, so multi-byte UTF-8 must
    /// survive unchanged on both dialects — no truncation, no re-encoding.
    #[test]
    fn multibyte_utf8_passes_through_unchanged() {
        let v = "naïve 日本";
        assert_eq!(quote_string(Engine::Trino, v), "'naïve 日本'");
        assert_eq!(quote_string(Engine::StarRocks, v), "'naïve 日本'");
    }

    /// The injection payload this primitive exists for. On StarRocks `\'` is an
    /// escaped quote, so a quote-only escape would emit `'\' OR 1=1 -- '` and
    /// hand the engine live SQL; doubling the backslash first makes the quote
    /// inert. Trino does not process backslash escapes inside a standard
    /// literal, so doubling the quote alone already contains it there.
    #[test]
    fn injection_payload_is_contained() {
        let payload = r"\' OR 1=1 -- ";
        assert_eq!(quote_string(Engine::Trino, payload), r"'\'' OR 1=1 -- '");
        assert_eq!(
            quote_string(Engine::StarRocks, payload),
            r"'\\'' OR 1=1 -- '"
        );
    }

    /// The everyday case: an embedded quote is doubled (never backslash-escaped)
    /// on both dialects, which is what keeps the two substitution passes
    /// order-independent.
    #[test]
    fn embedded_quote_is_doubled() {
        assert_eq!(quote_string(Engine::Trino, "O'Brien"), "'O''Brien'");
        assert_eq!(quote_string(Engine::StarRocks, "O'Brien"), "'O''Brien'");
    }

    /// Regression guard, stated on its own: the Trino arm must NEVER double a
    /// backslash. Trino reads `\` literally, so doubling it would silently
    /// corrupt every Windows path and every regex pattern passed as a filter
    /// value — a data-corruption bug with no error to notice.
    #[test]
    fn trino_never_doubles_backslashes() {
        for v in [r"C:\Users\bob", r"\d+\s*", r"\\server\share", r"\"] {
            let out = quote_string(Engine::Trino, v);
            assert_eq!(
                out,
                format!("'{v}'"),
                "Trino doubled a backslash in {v:?} -> {out}"
            );
        }
    }
}
