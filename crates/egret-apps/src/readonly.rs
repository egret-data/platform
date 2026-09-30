//! Read-only gate for the Data Apps query gateway (spec §4.3, D6).
//!
//! Fail closed: anything this module cannot parse, or any construct it does not
//! positively recognise as a read, is rejected. It is defence in addition to
//! Ranger, not instead of it. Table functions are rejected because Trino's
//! `TABLE(<catalog>.system.query(...))` passes arbitrary SQL — including writes —
//! to the underlying database.

use std::ops::ControlFlow;

use sqlparser::ast::{Query, SetExpr, Statement, TableFactor, Visit, Visitor};
use sqlparser::dialect::{Dialect, GenericDialect, MySqlDialect};
use sqlparser::parser::Parser;

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
    let stmts = Parser::parse_sql(&*dialect.dialect(), sql)
        .map_err(|e| ReadOnlyError::Unparseable(e.to_string()))?;
    if stmts.len() != 1 {
        return Err(ReadOnlyError::NotExactlyOne(stmts.len()));
    }
    check_statement(&stmts[0])
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

fn check_query(q: &Query) -> Result<(), ReadOnlyError> {
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
                return ControlFlow::Break(ReadOnlyError::NotReadOnly(
                    "an unsupported query clause",
                ));
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
    fn known_limitations_stay_rejected() {
        // Legitimate reads that sqlparser 0.63 cannot parse, or parses into a
        // shape the gate does not trust. They stay rejected: do not weaken the gate.
        no("TABLE t");
        no("SELECT * FROM t FOR VERSION AS OF 123");
        no("SELECT * FROM t FOR TIMESTAMP AS OF TIMESTAMP '2024-01-01 00:00:00'");
        no("SELECT * FROM t MATCH_RECOGNIZE (PARTITION BY a ORDER BY b PATTERN (A) DEFINE A AS true)");
        no("SHOW FUNCTIONS");
        // UNNEST passes for Trino, but the MySQL dialect parses it as a table function.
        let unnest = "SELECT * FROM t CROSS JOIN UNNEST(t.arr) AS u(a)";
        assert_eq!(check_read_only(unnest, Trino), Ok(()));
        assert!(check_read_only(unnest, StarRocks).is_err());
    }
}
