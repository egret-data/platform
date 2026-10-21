//! The row limit a caller asks for, applied to the SQL the engine runs.
//!
//! # Why the limit is appended, not wrapped
//!
//! A single `SELECT` used to run as `SELECT * FROM (<query>) AS _egret_lim LIMIT n`.
//! Two things were wrong with that (KI-154, KI-081):
//!
//! - **Trino drops the ordering of a subquery.** An `ORDER BY` inside the
//!   wrapper is not part of the outer query, so the rows came back unsorted, in
//!   a different order on each run.
//! - **`LIMIT n` hid the rest of the result.** The engine returned exactly `n`
//!   rows, the connector's `n + 1` fetch never saw the extra row, and
//!   `truncated` was `false` for a cut result.
//!
//! So when the SQL parses as exactly one query with no limit of its own, the
//! limit is appended to that query: `<query>\nLIMIT n + 1`. The query's own
//! `ORDER BY` stays outermost, the user's lines keep their numbers (the limit
//! goes on a line after them), and the connector sees the `n + 1`-th row and
//! reports `truncated`. A query with its own `LIMIT`, `OFFSET` or `FETCH` is
//! sent unchanged: its author bounded it, and the connector's fetch cap still
//! decides `truncated`.
//!
//! # When the SQL cannot be read
//!
//! SQL that `sqlparser` cannot parse (an engine-specific construct) but that
//! starts with `SELECT`/`WITH` keeps the old wrapper, so the engine's work stays
//! bounded; it asks for `n + 1` rows so `truncated` is still exact, and the
//! engine's error positions are mapped back to the user's lines with
//! [`unshift_error_lines`]. Comment forms that the engines and `sqlparser` read
//! differently (`/*!` hints, nested `/*`) count as unreadable, so the decision
//! is never taken on text the engine sees otherwise.

use egret_query_api::Engine;
use sqlparser::ast::{SetExpr, Statement};
use sqlparser::dialect::{Dialect, GenericDialect, MySqlDialect};
use sqlparser::parser::Parser;
use sqlparser::tokenizer::{Location, Token, Tokenizer, Whitespace};

/// The SQL to run for a caller's row `limit`, and whether it was wrapped.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Limited {
    pub sql: String,
    /// `true` when the query was wrapped in a subquery that adds one line before
    /// the user's text; engine error positions then need [`unshift_error_lines`].
    pub wrapped: bool,
}

fn dialect(engine: Engine) -> Box<dyn Dialect> {
    match engine {
        Engine::Trino => Box::new(GenericDialect {}),
        Engine::StarRocks => Box::new(MySqlDialect {}),
    }
}

/// Apply the caller's row `limit` (clamped to `max_rows`) to `sql`. See the
/// module documentation for the three outcomes.
pub(crate) fn apply_sql_limit(
    sql: &str,
    engine: Engine,
    limit: Option<usize>,
    max_rows: usize,
) -> Limited {
    let unchanged = || Limited {
        sql: sql.to_string(),
        wrapped: false,
    };
    let Some(l) = limit else {
        return unchanged();
    };
    // One row past the cap, so the connector can tell a cut result.
    let fetch = l.clamp(1, max_rows).saturating_add(1);
    match read_single_query(sql, engine) {
        Readable::Query { body, own_limit } => {
            if own_limit {
                unchanged()
            } else {
                Limited {
                    sql: format!("{body}\nLIMIT {fetch}"),
                    wrapped: false,
                }
            }
        }
        Readable::NotAQuery => unchanged(),
        Readable::Unreadable => {
            let trimmed = sql.trim().trim_end_matches(';').trim();
            let lower = trimmed.to_ascii_lowercase();
            let is_select = lower.starts_with("select") || lower.starts_with("with");
            if is_select && !trimmed.contains(';') {
                Limited {
                    sql: format!("SELECT * FROM (\n{trimmed}\n) AS _egret_lim LIMIT {fetch}"),
                    wrapped: true,
                }
            } else {
                unchanged()
            }
        }
    }
}

enum Readable<'a> {
    /// Exactly one query. `body` is the text without its trailing `;`.
    Query {
        body: &'a str,
        own_limit: bool,
    },
    /// Parsed, but not one plain query (several statements, `SHOW`, `EXPLAIN`,
    /// a DML-in-CTE, a parenthesised top level whose ordering would again be a
    /// subquery's).
    NotAQuery,
    Unreadable,
}

fn read_single_query(sql: &str, engine: Engine) -> Readable<'_> {
    if sql.contains("/*!") {
        return Readable::Unreadable;
    }
    let d = dialect(engine);
    let Ok(tokens) = Tokenizer::new(&*d, sql).tokenize_with_location() else {
        return Readable::Unreadable;
    };
    let nested_comment = tokens.iter().any(|t| {
        matches!(&t.token, Token::Whitespace(Whitespace::MultiLineComment(c)) if c.contains("/*"))
    });
    if nested_comment {
        return Readable::Unreadable;
    }
    let body = strip_trailing_semicolons_at(sql, &tokens);
    let Ok(stmts) = Parser::parse_sql(&*d, body) else {
        return Readable::Unreadable;
    };
    let [Statement::Query(q)] = stmts.as_slice() else {
        return Readable::NotAQuery;
    };
    if !matches!(
        *q.body,
        SetExpr::Select(_) | SetExpr::SetOperation { .. } | SetExpr::Values(_)
    ) {
        return Readable::NotAQuery;
    }
    let own_limit = q.limit_clause.is_some()
        || q.fetch.is_some()
        || !q.locks.is_empty()
        || q.for_clause.is_some()
        || q.settings.is_some()
        || q.format_clause.is_some()
        || !q.pipe_operators.is_empty();
    Readable::Query { body, own_limit }
}

/// `sql` without the `;` (and anything but whitespace and comments after it)
/// that ends it. Text the tokenizer cannot read is only trimmed of a final `;`.
pub fn strip_trailing_semicolons(sql: &str, engine: Engine) -> &str {
    let d = dialect(engine);
    match Tokenizer::new(&*d, sql).tokenize_with_location() {
        Ok(tokens) => strip_trailing_semicolons_at(sql, &tokens),
        Err(_) => sql.trim_end().trim_end_matches(';').trim_end(),
    }
}

fn strip_trailing_semicolons_at<'a>(
    sql: &'a str,
    tokens: &[sqlparser::tokenizer::TokenWithSpan],
) -> &'a str {
    let mut end = sql.len();
    for t in tokens.iter().rev() {
        match &t.token {
            Token::Whitespace(_) | Token::EOF => {}
            Token::SemiColon => match byte_offset(sql, t.span.start) {
                Some(at) => end = at,
                None => break,
            },
            _ => break,
        }
    }
    sql[..end].trim_end()
}

/// The byte offset of a tokenizer `Location` (1-based line and character column).
fn byte_offset(sql: &str, loc: Location) -> Option<usize> {
    let line = usize::try_from(loc.line).ok()?.checked_sub(1)?;
    let col = usize::try_from(loc.column).ok()?.checked_sub(1)?;
    let line_start = if line == 0 {
        0
    } else {
        sql.match_indices('\n').nth(line - 1)?.0 + 1
    };
    let rest = &sql[line_start..];
    match rest.char_indices().nth(col) {
        Some((i, _)) => Some(line_start + i),
        None => (rest.chars().count() == col).then_some(sql.len()),
    }
}

/// Map the error positions of an engine that ran the user's SQL one line down
/// (inside a `SELECT * FROM (\n…` wrapper) back to the user's lines: Trino's
/// `line 2:14:` and StarRocks' `at line 2, column 14` become line 1.
pub fn unshift_error_lines(msg: &str) -> String {
    let mut out = String::with_capacity(msg.len());
    let mut rest = msg;
    while let Some(at) = rest.find("line ") {
        let (head, tail) = rest.split_at(at + "line ".len());
        out.push_str(head);
        let digits = tail.bytes().take_while(u8::is_ascii_digit).count();
        let after = &tail[digits..];
        let is_position = digits > 0 && (after.starts_with(':') || after.starts_with(", column"));
        match tail[..digits].parse::<u64>() {
            Ok(n) if is_position && n > 1 => out.push_str(&(n - 1).to_string()),
            _ => out.push_str(&tail[..digits]),
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use Engine::{StarRocks, Trino};

    fn sql(s: &str, limit: usize) -> String {
        apply_sql_limit(s, Trino, Some(limit), 100).sql
    }

    #[test]
    fn a_plain_select_gets_the_limit_appended_with_one_extra_row() {
        assert_eq!(sql("SELECT * FROM t", 5), "SELECT * FROM t\nLIMIT 6");
    }

    #[test]
    fn order_by_stays_the_outermost_clause() {
        let out = sql(
            "SELECT nationkey, name FROM tpch.tiny.nation ORDER BY name DESC",
            5,
        );
        assert_eq!(
            out,
            "SELECT nationkey, name FROM tpch.tiny.nation ORDER BY name DESC\nLIMIT 6"
        );
        assert!(!out.contains("_egret_lim"), "{out}");
    }

    #[test]
    fn a_cte_and_a_union_get_the_limit_on_the_whole_query() {
        assert_eq!(
            sql("WITH a AS (SELECT 1 AS x) SELECT x FROM a ORDER BY x", 10),
            "WITH a AS (SELECT 1 AS x) SELECT x FROM a ORDER BY x\nLIMIT 11"
        );
        assert_eq!(
            sql("SELECT 1 AS x UNION ALL SELECT 2 ORDER BY x DESC", 10),
            "SELECT 1 AS x UNION ALL SELECT 2 ORDER BY x DESC\nLIMIT 11"
        );
    }

    #[test]
    fn a_trailing_semicolon_and_comments_are_handled() {
        assert_eq!(sql("SELECT 1;", 5), "SELECT 1\nLIMIT 6");
        assert_eq!(sql("SELECT 1 ; -- done\n", 5), "SELECT 1\nLIMIT 6");
        assert_eq!(sql("SELECT 1;;", 5), "SELECT 1\nLIMIT 6");
        // A trailing line comment is ended by the newline before the limit.
        assert_eq!(
            sql("SELECT 1 -- the one", 5),
            "SELECT 1 -- the one\nLIMIT 6"
        );
        assert_eq!(sql("SELECT 1 /* c */", 5), "SELECT 1 /* c */\nLIMIT 6");
    }

    #[test]
    fn a_query_with_its_own_limit_offset_or_fetch_is_left_alone() {
        for s in [
            "SELECT * FROM t ORDER BY a LIMIT 3",
            "SELECT * FROM t ORDER BY a OFFSET 2",
            "SELECT * FROM t ORDER BY a FETCH FIRST 3 ROWS ONLY",
        ] {
            assert_eq!(sql(s, 5), s);
        }
    }

    #[test]
    fn non_queries_and_several_statements_are_left_alone() {
        for s in [
            "SHOW CATALOGS",
            "EXPLAIN SELECT 1",
            "SELECT 1; SELECT 2",
            "CREATE TABLE t AS SELECT 1",
            "(SELECT a FROM t ORDER BY a)",
        ] {
            assert_eq!(sql(s, 5), s, "{s}");
        }
    }

    #[test]
    fn the_limit_is_clamped_to_max_rows() {
        assert_eq!(sql("SELECT 1", 999), "SELECT 1\nLIMIT 101");
    }

    #[test]
    fn no_limit_is_unchanged() {
        assert_eq!(
            apply_sql_limit("SELECT 1", Trino, None, 100),
            Limited {
                sql: "SELECT 1".into(),
                wrapped: false
            }
        );
    }

    #[test]
    fn unreadable_select_keeps_the_wrapper_with_one_extra_row() {
        // Nested comment: GenericDialect nests, Trino does not; not decided on.
        let s = "SELECT 1 /* a /* b */";
        let out = apply_sql_limit(s, Trino, Some(5), 100);
        assert!(out.wrapped);
        assert!(out.sql.ends_with("LIMIT 6"), "{}", out.sql);
        assert_eq!(
            apply_sql_limit("SELECT 1 /*! LIMIT 1 */", StarRocks, Some(5), 100).sql,
            "SELECT * FROM (\nSELECT 1 /*! LIMIT 1 */\n) AS _egret_lim LIMIT 6"
        );
    }

    #[test]
    fn starrocks_uses_its_own_dialect() {
        assert_eq!(
            apply_sql_limit("SELECT `a` FROM t ORDER BY `a`;", StarRocks, Some(5), 100).sql,
            "SELECT `a` FROM t ORDER BY `a`\nLIMIT 6"
        );
        assert_eq!(
            apply_sql_limit("SELECT * FROM t LIMIT 2, 3", StarRocks, Some(5), 100).sql,
            "SELECT * FROM t LIMIT 2, 3"
        );
    }

    #[test]
    fn multibyte_text_before_the_semicolon_is_cut_on_a_char_boundary() {
        assert_eq!(sql("SELECT 'é—ü' AS x;", 5), "SELECT 'é—ü' AS x\nLIMIT 6");
        assert_eq!(
            sql("SELECT 'é'\n, 'ü' AS y ;", 5),
            "SELECT 'é'\n, 'ü' AS y\nLIMIT 6"
        );
    }

    #[test]
    fn strip_trailing_semicolons_keeps_the_body() {
        assert_eq!(
            strip_trailing_semicolons("select 1 as x;", Trino),
            "select 1 as x"
        );
        assert_eq!(
            strip_trailing_semicolons("select ';' ; ", Trino),
            "select ';'"
        );
        assert_eq!(strip_trailing_semicolons("select 1", StarRocks), "select 1");
    }

    #[test]
    fn error_lines_are_moved_back_by_the_wrapper_line() {
        assert_eq!(
            unshift_error_lines("line 2:22: Table 'tpch.tiny.ordres' does not exist"),
            "line 1:22: Table 'tpch.tiny.ordres' does not exist"
        );
        assert_eq!(
            unshift_error_lines("Getting syntax error at line 3, column 7. Detail"),
            "Getting syntax error at line 2, column 7. Detail"
        );
        // Not a position: left alone.
        assert_eq!(
            unshift_error_lines("column 'line 2' missing"),
            "column 'line 2' missing"
        );
        assert_eq!(unshift_error_lines("line 1:1: x"), "line 1:1: x");
    }
}
