//! Read-only gate for the Data Apps query gateway (spec §4.3, D6).
//!
//! Fail closed: anything this module cannot parse, or any construct it does not
//! positively recognise as a read, is rejected. It is defence in addition to
//! Ranger, not instead of it. Table functions are rejected because Trino's
//! `TABLE(<catalog>.system.query(...))` passes arbitrary SQL — including writes —
//! to the underlying database.
//!
//! # Lexer equivalence
//!
//! The gate is only sound if it sees the same tokens the engine executes: a
//! comment or string the gate skips over but the engine runs (or the reverse)
//! is a bypass, e.g. `/* /* */ DELETE FROM t -- */ SELECT 1`. sqlparser's
//! `GenericDialect` / `MySqlDialect` lex more liberally than Trino / StarRocks, so
//! the gate narrows them to forms both sides read alike and rejects the rest:
//!
//! - any `/*!` in the text is rejected before parsing: MySQL-family engines
//!   may execute the comment body, Trino ignores it, and both stock sqlparser
//!   dialects expand it into SQL;
//! - a `/* … */` comment containing `/*` is rejected: `GenericDialect` nests
//!   comments and Trino does not, but without an inner `/*` nesting cannot
//!   change where a comment ends;
//! - after tokenizing, only plain string literals (`'…'`, plus `"…"` and `X'…'`
//!   where the engine has them) and the engine's own identifier quoting are
//!   allowed; `#` comments and (StarRocks) `--` without a following space are
//!   rejected, because the two lexers disagree on whether they start a comment;
//!   so is a `\r` followed by anything but line breaks inside a `--` comment
//!   (the engines end the comment at `\r`, sqlparser only at `\n`).
//!
//! The stock dialects are used deliberately: sqlparser's `derive_dialect!`
//! wrapper reads the trait source from the cargo registry at compile time,
//! which is not hermetic. These token rules make the two lexer flags that
//! matter (comment nesting, `/*!` hints) unable to change what the gate sees.

use std::ops::ControlFlow;

use sqlparser::ast::{Query, SetExpr, Statement, TableFactor, Visit, Visitor};
use sqlparser::dialect::{Dialect, GenericDialect, MySqlDialect};
use sqlparser::parser::Parser;
use sqlparser::tokenizer::{Token, Tokenizer, Whitespace};

#[derive(Debug, Clone, Copy)]
pub enum EngineDialect {
    Trino,
    StarRocks,
}

impl EngineDialect {
    /// Maps the engine string stored in `query.databases.engine`.
    pub fn from_engine(engine: &str) -> Option<Self> {
        match engine {
            "trino" => Some(Self::Trino),
            "starrocks" => Some(Self::StarRocks),
            _ => None,
        }
    }

    fn dialect(self) -> Box<dyn Dialect> {
        match self {
            Self::Trino => Box::new(GenericDialect {}),
            Self::StarRocks => Box::new(MySqlDialect {}),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum ReadOnlyError {
    Unparseable(String),
    NotExactlyOne(usize),
    NotReadOnly(&'static str),
}

impl std::fmt::Display for ReadOnlyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unparseable(e) => write!(
                f,
                "the SQL could not be parsed, so it cannot be shown to be read-only ({e}); rewrite it in standard SQL"
            ),
            Self::NotExactlyOne(n) => {
                write!(f, "exactly one statement is allowed per query, got {n}")
            }
            Self::NotReadOnly(what) => {
                write!(f, "data apps may only read data: {what} is not allowed")
            }
        }
    }
}

impl std::error::Error for ReadOnlyError {}

/// Accepts `sql` only if it is exactly one statement that is positively known to read.
pub fn check_read_only(sql: &str, dialect: EngineDialect) -> Result<(), ReadOnlyError> {
    // Anywhere, even inside a string literal: fail closed.
    if sql.contains("/*!") {
        return Err(ReadOnlyError::NotReadOnly(
            "a MySQL-style executable comment (/*! … */)",
        ));
    }
    let lexer = dialect.dialect();
    check_tokens(sql, &*lexer, dialect)?;
    let stmts =
        Parser::parse_sql(&*lexer, sql).map_err(|e| ReadOnlyError::Unparseable(e.to_string()))?;
    if stmts.len() != 1 {
        return Err(ReadOnlyError::NotExactlyOne(stmts.len()));
    }
    check_statement(&stmts[0])
}

/// Rejects tokens whose extent the engine's lexer may draw differently from
/// sqlparser's: string/identifier quoting forms other than the engine's plain
/// ones, `#` comments, and (StarRocks) `--` glued to the next character.
fn check_tokens(
    sql: &str,
    lexer: &dyn Dialect,
    dialect: EngineDialect,
) -> Result<(), ReadOnlyError> {
    const QUOTING: &str = "a string or identifier quoting form the engine may read differently";
    let tokens = Tokenizer::new(lexer, sql)
        .tokenize()
        .map_err(|e| ReadOnlyError::Unparseable(e.to_string()))?;
    let mut prev_ends_dash = false;
    for token in &tokens {
        let ok = match (dialect, token) {
            (_, Token::SingleQuotedString(_) | Token::HexStringLiteral(_)) => true,
            (EngineDialect::StarRocks, Token::DoubleQuotedString(_)) => true,
            (_, Token::Word(w)) => matches!(
                (dialect, w.quote_style),
                (_, None)
                    | (EngineDialect::Trino, Some('"'))
                    | (EngineDialect::StarRocks, Some('`'))
            ),
            (_, Token::Whitespace(Whitespace::SingleLineComment { prefix, comment })) => {
                if prefix != "--" {
                    return Err(ReadOnlyError::NotReadOnly("a `#` comment"));
                }
                // sqlparser ends a `--` comment only at `\n`; Trino and StarRocks
                // also end it at `\r`, so text after a bare `\r` would run unseen.
                // A trailing `\r` (CRLF line ending) is harmless: only line
                // breaks follow it, so nothing runs unseen.
                if comment.trim_end_matches(['\r', '\n']).contains('\r') {
                    return Err(ReadOnlyError::NotReadOnly(
                        "a carriage return inside a `--` comment",
                    ));
                }
                true
            }
            (_, Token::Whitespace(Whitespace::MultiLineComment(body))) => {
                if body.contains("/*") {
                    return Err(ReadOnlyError::NotReadOnly(
                        "a `/*` inside a block comment (nesting differs between engines)",
                    ));
                }
                true
            }
            (
                _,
                Token::DoubleQuotedString(_)
                | Token::TripleSingleQuotedString(_)
                | Token::TripleDoubleQuotedString(_)
                | Token::DollarQuotedString(_)
                | Token::SingleQuotedByteStringLiteral(_)
                | Token::DoubleQuotedByteStringLiteral(_)
                | Token::TripleSingleQuotedByteStringLiteral(_)
                | Token::TripleDoubleQuotedByteStringLiteral(_)
                | Token::SingleQuotedRawStringLiteral(_)
                | Token::DoubleQuotedRawStringLiteral(_)
                | Token::TripleSingleQuotedRawStringLiteral(_)
                | Token::TripleDoubleQuotedRawStringLiteral(_)
                | Token::NationalStringLiteral(_)
                | Token::QuoteDelimitedStringLiteral(_)
                | Token::NationalQuoteDelimitedStringLiteral(_)
                | Token::EscapedStringLiteral(_)
                | Token::UnicodeStringLiteral(_),
            ) => false,
            _ => true,
        };
        if !ok {
            return Err(ReadOnlyError::NotReadOnly(QUOTING));
        }
        // MySqlDialect lexes `--x` as two tokens (MySQL needs `-- `): `-` `-`,
        // `-` `->`, `-` `->>`, …; the StarRocks lexer may start a comment there
        // and hide what follows. Reject any two adjacent tokens that join into
        // `--`, and any operator token that contains it.
        // Whitespace, words, numbers and the (already allow-listed) literals
        // cannot form `--`; their rendered text would include quote contents.
        let text = match token {
            Token::Whitespace(_)
            | Token::Word(_)
            | Token::Number(..)
            | Token::SingleQuotedString(_)
            | Token::HexStringLiteral(_)
            | Token::DoubleQuotedString(_) => String::new(),
            t => t.to_string(),
        };
        if (prev_ends_dash && text.starts_with('-')) || text.contains("--") {
            return Err(ReadOnlyError::NotReadOnly(
                "`--` not followed by whitespace",
            ));
        }
        prev_ends_dash = text.ends_with('-');
    }
    Ok(())
}

fn check_statement(stmt: &Statement) -> Result<(), ReadOnlyError> {
    match stmt {
        Statement::Query(q) => check_query(q),
        // `EXPLAIN ANALYZE` executes its inner statement, so the inner statement is gated too.
        Statement::Explain { statement, .. } => check_statement(statement),
        // `DESCRIBE t` / `DESC t` / `EXPLAIN t`.
        Statement::ExplainTable { .. } => Ok(()),
        Statement::ShowTables { .. }
        | Statement::ShowColumns { .. }
        | Statement::ShowCreate { .. }
        | Statement::ShowSchemas { .. }
        | Statement::ShowDatabases { .. }
        | Statement::ShowCatalogs { .. }
        | Statement::ShowViews { .. } => Ok(()),
        _ => Err(ReadOnlyError::NotReadOnly("this statement kind")),
    }
}

/// Walks every query, sub-query and table factor reachable from a query.
/// Its layers overlap on purpose: with sqlparser 0.63 a statement can only nest
/// inside a query as a data-modifying `SetExpr`, which `pre_visit_query` already
/// refuses; `pre_visit_statement` keeps refusing if a later parser nests one elsewhere.
struct Guard;

impl Visitor for Guard {
    type Break = ReadOnlyError;

    fn pre_visit_query(&mut self, q: &Query) -> ControlFlow<Self::Break> {
        if !q.locks.is_empty() {
            return ControlFlow::Break(ReadOnlyError::NotReadOnly("a locking clause"));
        }
        // Dialect-specific trailing clauses (MSSQL `FOR XML`, ClickHouse
        // `SETTINGS` / `FORMAT`, BigQuery pipe syntax) are not Trino or
        // StarRocks reads we need; reject rather than reason about them.
        if q.for_clause.is_some()
            || q.settings.is_some()
            || q.format_clause.is_some()
            || !q.pipe_operators.is_empty()
        {
            return ControlFlow::Break(ReadOnlyError::NotReadOnly("an unsupported query clause"));
        }
        match &*q.body {
            SetExpr::Select(s) if s.into.is_some() => {
                ControlFlow::Break(ReadOnlyError::NotReadOnly("SELECT … INTO"))
            }
            SetExpr::Select(_)
            | SetExpr::Query(_)
            | SetExpr::SetOperation { .. }
            | SetExpr::Values(_)
            | SetExpr::Table(_) => ControlFlow::Continue(()),
            _ => ControlFlow::Break(ReadOnlyError::NotReadOnly("a data-modifying query body")),
        }
    }

    fn pre_visit_table_factor(&mut self, t: &TableFactor) -> ControlFlow<Self::Break> {
        match t {
            TableFactor::Table { args: None, .. }
            | TableFactor::Derived { .. }
            | TableFactor::NestedJoin { .. }
            | TableFactor::UNNEST { .. } => ControlFlow::Continue(()),
            _ => ControlFlow::Break(ReadOnlyError::NotReadOnly("a table function")),
        }
    }

    fn pre_visit_statement(&mut self, _: &Statement) -> ControlFlow<Self::Break> {
        ControlFlow::Break(ReadOnlyError::NotReadOnly("a statement nested in a query"))
    }
}

fn check_query(q: &Query) -> Result<(), ReadOnlyError> {
    match q.visit(&mut Guard) {
        ControlFlow::Break(e) => Err(e),
        ControlFlow::Continue(()) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use EngineDialect::*;

    fn ok(sql: &str) {
        for d in [Trino, StarRocks] {
            assert_eq!(check_read_only(sql, d), Ok(()), "{d:?}: {sql}");
        }
    }
    fn no(sql: &str) {
        for d in [Trino, StarRocks] {
            assert!(check_read_only(sql, d).is_err(), "{d:?} accepted: {sql}");
        }
    }
    /// Refused by the gate itself, not merely by the parser: pins that the
    /// construct parses and that the gate is what rejects it.
    fn gated(sql: &str) {
        for d in [Trino, StarRocks] {
            let got = check_read_only(sql, d);
            assert!(
                matches!(got, Err(ReadOnlyError::NotReadOnly(_))),
                "{d:?}: {sql} -> {got:?}"
            );
        }
    }

    #[test]
    fn plain_reads_pass() {
        ok("SELECT 1");
        ok("SELECT 1;");
        ok("select a, count(*) from t where b > 1 group by a order by 2 desc limit 10");
        ok("WITH x AS (SELECT * FROM t) SELECT * FROM x JOIN u ON x.id = u.id");
        ok("SELECT * FROM (SELECT 1 AS a) s UNION ALL SELECT 2");
        ok("-- a comment\nSELECT /* inline */ 1");
        ok("SELECT * FROM cat.sch.t");
    }
    #[test]
    fn metadata_reads_pass() {
        ok("EXPLAIN SELECT * FROM t");
        ok("DESCRIBE t");
        ok("SHOW TABLES");
        ok("SHOW COLUMNS FROM t");
    }
    #[test]
    fn writes_and_ddl_are_rejected() {
        no("INSERT INTO t VALUES (1)");
        no("UPDATE t SET a = 1");
        no("DELETE FROM t");
        no("MERGE INTO t USING s ON t.id = s.id WHEN MATCHED THEN DELETE");
        no("CREATE TABLE t2 AS SELECT * FROM t");
        no("DROP TABLE t");
        no("TRUNCATE TABLE t");
        no("GRANT SELECT ON t TO u");
        no("CALL system.flush_metadata_cache()");
        no("SET SESSION query_max_run_time = '1h'");
        no("USE other_schema");
    }
    #[test]
    fn smuggling_is_rejected() {
        no("SELECT 1; DROP TABLE t");
        no("/* harmless */ DELETE FROM t");
        no("EXPLAIN DELETE FROM t");
        no("SELECT * INTO t2 FROM t");
        no("SELECT * FROM t FOR UPDATE");
        no("WITH x AS (DELETE FROM t RETURNING *) SELECT * FROM x");
    }
    #[test]
    fn table_functions_are_rejected() {
        // Trino pass-through: runs arbitrary SQL on the underlying database.
        no("SELECT * FROM TABLE(pg.system.query(query => 'DELETE FROM t'))");
        no("SELECT * FROM my_table_function(1)");
    }
    #[test]
    fn unparseable_and_empty_are_rejected() {
        no("");
        no("   ");
        no("SELEC 1");
        no("this is not sql");
    }
    #[test]
    fn error_kinds() {
        assert!(matches!(
            check_read_only("SELECT 1; SELECT 2", Trino),
            Err(ReadOnlyError::NotExactlyOne(2))
        ));
        assert!(matches!(
            check_read_only("SELEC 1", Trino),
            Err(ReadOnlyError::Unparseable(_))
        ));
        assert!(matches!(
            check_read_only("DELETE FROM t", Trino),
            Err(ReadOnlyError::NotReadOnly(_))
        ));
    }
    #[test]
    fn dialect_from_engine() {
        assert!(matches!(EngineDialect::from_engine("trino"), Some(Trino)));
        assert!(matches!(
            EngineDialect::from_engine("starrocks"),
            Some(StarRocks)
        ));
        assert!(EngineDialect::from_engine("postgres").is_none());
    }

    #[test]
    fn each_guard_layer_refuses_on_its_own() {
        // The query-body and nested-statement layers mask each other end to end,
        // so pin each directly.
        let del = Parser::parse_sql(&GenericDialect {}, "DELETE FROM t")
            .unwrap()
            .remove(0);
        assert!(matches!(
            Guard.pre_visit_statement(&del),
            ControlFlow::Break(ReadOnlyError::NotReadOnly(_))
        ));
        let Statement::Query(q) = Parser::parse_sql(
            &GenericDialect {},
            "WITH x AS (DELETE FROM t RETURNING *) SELECT * FROM x",
        )
        .unwrap()
        .remove(0) else {
            panic!("expected a query")
        };
        let cte_body = &q.with.as_ref().unwrap().cte_tables[0].query;
        assert!(matches!(
            Guard.pre_visit_query(cte_body),
            ControlFlow::Break(ReadOnlyError::NotReadOnly(_))
        ));
    }
    #[test]
    fn rejections_come_from_the_gate_not_the_parser() {
        // Every write/smuggling case above parses in both dialects, so the gate
        // (not a parse failure) must be what refuses it.
        for sql in [
            "INSERT INTO t VALUES (1)",
            "UPDATE t SET a = 1",
            "DELETE FROM t",
            "MERGE INTO t USING s ON t.id = s.id WHEN MATCHED THEN DELETE",
            "CREATE TABLE t2 AS SELECT * FROM t",
            "DROP TABLE t",
            "TRUNCATE TABLE t",
            "GRANT SELECT ON t TO u",
            "CALL system.flush_metadata_cache()",
            "SET SESSION query_max_run_time = '1h'",
            "USE other_schema",
            "/* harmless */ DELETE FROM t",
            "EXPLAIN DELETE FROM t",
            "SELECT * INTO t2 FROM t",
            "SELECT * FROM t FOR UPDATE",
            "WITH x AS (DELETE FROM t RETURNING *) SELECT * FROM x",
            "SELECT * FROM TABLE(pg.system.query(query => 'DELETE FROM t'))",
            "SELECT * FROM my_table_function(1)",
        ] {
            gated(sql);
        }
    }
    #[test]
    fn explain_is_only_as_safe_as_its_inner_statement() {
        gated("EXPLAIN ANALYZE DELETE FROM t");
        gated("EXPLAIN WITH x AS (DELETE FROM t RETURNING *) SELECT * FROM x");
        gated("EXPLAIN SELECT * FROM TABLE(pg.system.query(query => 'DELETE FROM t'))");
        ok("EXPLAIN ANALYZE SELECT * FROM t");
        ok("EXPLAIN t");
    }
    #[test]
    fn table_functions_are_rejected_wherever_they_hide() {
        gated("WITH x AS (SELECT * FROM TABLE(pg.system.query(query => 'DELETE FROM t'))) SELECT * FROM x");
        gated("WITH x AS (SELECT * FROM f(1)) SELECT * FROM x");
        gated("SELECT * FROM t WHERE a IN (SELECT * FROM TABLE(pg.system.query(query => 'DELETE FROM t')))");
        gated("SELECT * FROM t WHERE EXISTS (SELECT 1 FROM f(1))");
        gated("SELECT (SELECT max(x) FROM f(1)) FROM t");
        gated("SELECT * FROM t JOIN f(1) ON true");
        gated("SELECT * FROM t CROSS JOIN LATERAL f(t.x) s");
        gated("SELECT * FROM t CROSS JOIN LATERAL (SELECT * FROM f(1)) s");
        gated("SELECT * FROM (SELECT * FROM (SELECT * FROM TABLE(q(1))) a) b");
        gated("SELECT 1 UNION SELECT * FROM f(1)");
        gated("SELECT * FROM JSON_TABLE('[]', '$[*]' COLUMNS (a INT PATH '$.a')) AS j");
    }
    #[test]
    fn other_reads_pass() {
        ok("VALUES (1)");
        ok("(SELECT 1) UNION (SELECT 2)");
        ok("SELECT * FROM t, LATERAL (SELECT * FROM u WHERE u.id = t.id) s");
        ok("SELECT * FROM t CROSS JOIN LATERAL (SELECT * FROM u) s");
        ok("SELECT * FROM (t JOIN u ON t.id = u.id)");
        ok("SELECT * FROM t TABLESAMPLE BERNOULLI (10)");
        ok("SHOW CREATE TABLE t");
        ok("SHOW SCHEMAS");
        ok("SHOW CATALOGS");
        ok("DESC cat.sch.t");
        ok("SELECT 1;;");
    }
    #[test]
    fn other_statements_and_clauses_are_rejected() {
        gated("SELECT * FROM t FOR SHARE");
        gated("SHOW PROCESSLIST");
        gated("SHOW VARIABLES");
        gated("EXECUTE p");
        gated("COMMIT");
        gated("KILL 1");
        gated("ANALYZE TABLE t");
        gated("ALTER TABLE t ADD COLUMN c INT");
        gated("INSERT INTO t SELECT * FROM u");
        // ClickHouse / BigQuery trailing clauses parse under the generic (Trino) dialect.
        for sql in [
            "SELECT * FROM t SETTINGS max_threads = 1",
            "SELECT * FROM t FORMAT JSON",
            "SELECT * FROM t |> WHERE a > 1",
        ] {
            let got = check_read_only(sql, Trino);
            assert!(
                matches!(got, Err(ReadOnlyError::NotReadOnly(_))),
                "{sql} -> {got:?}"
            );
        }
        no("SELECT * INTO OUTFILE '/tmp/x' FROM t");
        no("SELECT a FROM t INTO @v");
        no("PREPARE p FROM 'DELETE FROM t'");
        assert_eq!(
            check_read_only(";", Trino),
            Err(ReadOnlyError::NotExactlyOne(0))
        );
    }
    #[test]
    fn lexer_differentials_are_rejected() {
        // Nested comments: Trino ends a bracketed comment at the first `*/`.
        gated("/* /* */ DELETE FROM t -- */ SELECT 1");
        gated("/* /* */ DROP TABLE t -- */ SELECT 1");
        gated("/* a /* b */ DROP TABLE t; -- */ SELECT 1");
        // `/*! … */`: plain comment to Trino, executable to the MySQL family.
        gated("/*! SELECT */ DELETE FROM t");
        gated("/*!99999 SELECT */ DELETE FROM t");
        gated("/*! DELETE FROM t */ SELECT 1");
        gated("/*!50000 DELETE FROM t */ SELECT 1");
        // `--` comment cut short by a bare carriage return in the engines.
        gated(
            "SELECT 1 -- c\rUNION SELECT * FROM TABLE(pg.system.query(query => 'DELETE FROM t'))",
        );
        gated("SELECT 1 -- c\r; DELETE FROM t");
        // `#` comments: a comment to MySqlDialect, not to StarRocks.
        let hash = check_read_only("SELECT 1 # c\nFROM t", StarRocks);
        assert!(
            matches!(hash, Err(ReadOnlyError::NotReadOnly(_))),
            "{hash:?}"
        );
        // `--x`: two minus signs to MySqlDialect, a comment to the StarRocks lexer.
        for sql in ["SELECT 1 --x", "SELECT 1--1", "SELECT a--b FROM t"] {
            let got = check_read_only(sql, StarRocks);
            assert!(
                matches!(got, Err(ReadOnlyError::NotReadOnly(_))),
                "{sql} -> {got:?}"
            );
        }
    }
    #[test]
    fn the_gate_rejects_what_the_bare_parser_accepts() {
        // Pins why the token rules and the pre-check exist: the stock dialects
        // alone read these as one harmless SELECT; the gate refuses them.
        for (sql, base) in [
            (
                "/* /* */ DELETE FROM t -- */ SELECT 1",
                &GenericDialect {} as &dyn Dialect,
            ),
            ("/*! SELECT */ DELETE FROM t", &GenericDialect {}),
            ("/*! SELECT */ DELETE FROM t", &MySqlDialect {}),
            ("SELECT 1 -- c\r; DELETE FROM t", &GenericDialect {}),
        ] {
            let stmts = Parser::parse_sql(base, sql).unwrap();
            assert!(
                matches!(stmts.as_slice(), [Statement::Query(_)]),
                "{sql}: {stmts:?}"
            );
            gated(sql);
        }
    }
    #[test]
    fn engine_lexing_is_respected() {
        // Trino has no backslash escapes: `'a\'` is the string `a\`.
        assert_eq!(check_read_only(r"SELECT 'a\' FROM t", Trino), Ok(()));
        // StarRocks does: the quote is escaped, the literal never ends, and the
        // engine would fail the same way.
        assert!(matches!(
            check_read_only(r"SELECT 'a\' FROM t", StarRocks),
            Err(ReadOnlyError::Unparseable(_))
        ));
        ok("SELECT 'it''s', X'AB', \"a\" FROM t");
        ok("SELECT 1 -- a comment with a space\nFROM t");
        ok("SELECT 1 - -1");
        ok("SELECT /*+ SET_VAR(a = 1) */ 1 /* trailing */");
        assert_eq!(check_read_only("SELECT 1 --x", Trino), Ok(()));
        assert_eq!(check_read_only("SELECT `a` FROM t", StarRocks), Ok(()));
        // Quoting forms whose extent the engine may draw differently.
        for sql in [
            "SELECT E'a\\' FROM t",
            "SELECT N'x'",
            "SELECT b'x'",
            "SELECT `a` FROM t",
            "SELECT U&'x'",
            "SELECT r'x'",
            "SELECT q'[x]'",
            "SELECT $$x$$",
        ] {
            assert!(check_read_only(sql, Trino).is_err(), "Trino accepted {sql}");
        }
        for sql in ["SELECT N'x'", "SELECT b'x'"] {
            assert!(
                check_read_only(sql, StarRocks).is_err(),
                "StarRocks accepted {sql}"
            );
        }
    }
    #[test]
    fn a_slash_star_inside_a_block_comment_is_rejected() {
        // With no `/*` inside a comment, nesting cannot move its end, so the
        // stock dialects' nesting flag cannot matter.
        // Balanced for a nesting lexer, so both dialects tokenize it and the
        // comment rule (not a parse error) is what refuses it.
        gated("SELECT 1 /* /* */ */");
        gated("/* a /* b */ c */ SELECT 1");
        // Unbalanced for a nesting lexer: unterminated under GenericDialect.
        no("SELECT 1 /* a /* b */");
        no("SELECT /* /* */ 1");
        ok("SELECT /* a * b / c */ 1");
        ok("SELECT 1 /* x */ /* y */");
    }
    #[test]
    fn crlf_line_endings_are_fine_but_a_bare_cr_is_not() {
        ok("SELECT 1 -- c\r\nUNION SELECT 2");
        ok("SELECT 1 --\r\nFROM t");
        ok("SELECT 1 -- c\r\n\r\nFROM t");
        ok("SELECT 1 -- trailing\r\n");
        gated("SELECT 1 -- c\rUNION SELECT 2");
        gated("SELECT 1 -- c\r\rFROM t\n");
    }
    #[test]
    fn dashes_that_join_into_a_comment_marker_are_rejected() {
        for sql in [
            "SELECT a-->1 FROM t",
            "SELECT a-->>'x' FROM t",
            "SELECT a --> 1 FROM t",
            "SELECT 1 --x",
            "SELECT 1--1",
        ] {
            let got = check_read_only(sql, StarRocks);
            assert!(
                matches!(got, Err(ReadOnlyError::NotReadOnly(_))),
                "{sql} -> {got:?}"
            );
        }
        ok("SELECT a - -1, a - (-1), a -> 'x' FROM t");
        ok("SELECT '--x', \"a--b\" FROM t");
    }
    #[test]
    fn known_limitations_stay_rejected() {
        // Legitimate reads that sqlparser 0.63 cannot parse, or parses into a
        // shape the gate does not trust. They stay rejected: do not weaken the gate.
        no("TABLE t");
        no("SELECT * FROM t FOR VERSION AS OF 123");
        no("SELECT * FROM t FOR TIMESTAMP AS OF TIMESTAMP '2024-01-01 00:00:00'");
        no("SELECT * FROM t MATCH_RECOGNIZE (PARTITION BY a ORDER BY b PATTERN (A) DEFINE A AS true)");
        no("SHOW FUNCTIONS");
        // `/*!` is refused anywhere, even inside a string literal.
        gated("SELECT '/*!' FROM t");
        // UNNEST passes for Trino, but the MySQL dialect parses it as a table function.
        let unnest = "SELECT * FROM t CROSS JOIN UNNEST(t.arr) AS u(a)";
        assert_eq!(check_read_only(unnest, Trino), Ok(()));
        assert!(check_read_only(unnest, StarRocks).is_err());
    }
}
