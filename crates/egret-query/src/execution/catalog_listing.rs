//! DG frontend redesign #1 (Governance Data): list one level of a catalog on a
//! Trino engine, as `egret-lineage` — the Trino half of the browse route's
//! `CatalogLister` (`egret_controlplane::governance_browse`; `egret-bin` wires
//! it).
//!
//! # Identity and engine
//!
//! The same as the lineage EXPLAIN ([`super::lineage`]): an engine JWT for
//! `egret-lineage` that only egret can mint, presented over the engine's TLS
//! listener, on an engine [`lineage_engines_via`] returns for the catalog —
//! provisioned, running, JWT delegation mode, attachment applied, with a
//! delegated connection. The download-time lineage transform already serves
//! that principal `use` + `select` on every attached catalog, and Trino 483
//! checks `_any` for a listing, so no new grant is needed (measured,
//! `docs/en/plans/2026-10-16-dg-frontend-data-live.md` §1.1).
//!
//! # Statements (§1.4)
//!
//! - schemas: `SHOW SCHEMAS FROM "c"`, without [`is_hidden_schema`] ones;
//! - relations: `SELECT table_name, table_type FROM "c".information_schema.tables
//!   WHERE table_schema = 's'` — the only measured form that carries the kind;
//! - columns: `DESCRIBE "c"."s"."t"` (`Column, Type, Extra, Comment`).
//!
//! The catalog, schema and table are untrusted: each is quoted with
//! [`quote_ident`] or, as the schema literal, [`quote_string`]. Trino's
//! statement protocol has no `?` binding.
//!
//! # Empty is not "missing"
//!
//! `information_schema.tables` answers an empty set for a schema that does not
//! exist. So when it is empty the lister asks `SHOW TABLES FROM "c"."s"`, which
//! fails with `SCHEMA_NOT_FOUND` for a missing schema and answers an empty set
//! for a real empty one (J3). `DESCRIBE` fails with `TABLE_NOT_FOUND` for a
//! missing table; a `DESCRIBE` with no columns (a view Trino cannot read, J13) is
//! an error, never an empty column list.

use std::sync::Arc;
use std::time::Duration;

use egret_core::TabularResult;
use egret_perm_compile::lineage::LINEAGE_PRINCIPAL;
use egret_query_api::catalog_listing::{
    is_hidden_schema, relation_kind, CatalogLevel, ListedColumn, ListedRelation,
};
use egret_query_api::{quote_ident, quote_string, ConnOptions, Engine};
use serde_json::Value;

use crate::execution::connector::{run_catalog_listing, ConnSpec};
use crate::execution::lineage::{lineage_engines_via, LineageTokenMinter};

/// The most rows one listing reads. A level with more is an error rather than
/// a silently shortened list.
pub const MAX_LISTED: usize = 10_000;

/// `SHOW SCHEMAS FROM "c"`.
pub fn schemas_sql(catalog: &str) -> String {
    format!("SHOW SCHEMAS FROM {}", ident(catalog))
}

/// The schema's relations with their `table_type`.
pub fn relations_sql(catalog: &str, schema: &str) -> String {
    format!(
        "SELECT table_name, table_type FROM {}.information_schema.tables \
         WHERE table_schema = {}",
        ident(catalog),
        quote_string(Engine::Trino, schema)
    )
}

/// `SHOW TABLES FROM "c"."s"`: fails for a missing schema (module doc).
pub fn schema_probe_sql(catalog: &str, schema: &str) -> String {
    format!("SHOW TABLES FROM {}.{}", ident(catalog), ident(schema))
}

/// `DESCRIBE "c"."s"."t"`.
pub fn columns_sql(catalog: &str, schema: &str, table: &str) -> String {
    format!(
        "DESCRIBE {}.{}.{}",
        ident(catalog),
        ident(schema),
        ident(table)
    )
}

fn ident(name: &str) -> String {
    quote_ident(Engine::Trino, name)
}

/// The rows of a complete answer; a truncated one is an error.
fn complete(result: &TabularResult) -> Result<&[Vec<Value>], String> {
    if result.truncated {
        return Err(format!(
            "the engine answered more than {MAX_LISTED} rows; egret does not list a shortened level"
        ));
    }
    Ok(&result.rows)
}

/// The text in cell `i` of `row`.
fn text(row: &[Value], i: usize, what: &str) -> Result<String, String> {
    match row.get(i) {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(other) => Err(format!("the engine answered {other} as a {what}")),
        None => Err(format!("the engine answered a row with no {what}")),
    }
}

/// The schemas out of `SHOW SCHEMAS` (column `Schema`), sorted, hidden ones
/// dropped.
pub fn parse_schemas(result: &TabularResult) -> Result<Vec<String>, String> {
    let mut schemas = Vec::new();
    for row in complete(result)? {
        let name = text(row, 0, "schema name")?;
        if !is_hidden_schema(Engine::Trino, &name) {
            schemas.push(name);
        }
    }
    schemas.sort();
    Ok(schemas)
}

/// The relations out of [`relations_sql`]'s answer, sorted by name.
pub fn parse_relations(result: &TabularResult) -> Result<Vec<ListedRelation>, String> {
    let mut relations = complete(result)?
        .iter()
        .map(|row| {
            Ok(ListedRelation {
                name: text(row, 0, "table name")?,
                kind: relation_kind(&text(row, 1, "table type")?)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    relations.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(relations)
}

/// The columns out of `DESCRIBE` (`Column, Type, Extra, Comment`), in order.
/// No column at all is an error (module doc).
pub fn parse_columns(result: &TabularResult) -> Result<Vec<ListedColumn>, String> {
    let columns = complete(result)?
        .iter()
        .map(|row| {
            Ok(ListedColumn {
                name: text(row, 0, "column name")?,
                r#type: text(row, 1, "column type")?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    if columns.is_empty() {
        return Err("the engine described no column".to_string());
    }
    Ok(columns)
}

/// One level of `catalog`, each statement sent through `run` (the engine, or
/// a test's fake). See the module doc for why an empty relation listing is
/// followed by [`schema_probe_sql`].
pub(crate) async fn list_level<F, Fut>(
    run: F,
    catalog: &str,
    schema: Option<&str>,
    table: Option<&str>,
) -> Result<CatalogLevel, String>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<TabularResult, String>>,
{
    match (schema, table) {
        (None, None) => Ok(CatalogLevel::Schemas(parse_schemas(
            &run(schemas_sql(catalog)).await?,
        )?)),
        (None, Some(_)) => Err("a table needs a schema".to_string()),
        (Some(schema), None) => {
            let relations = parse_relations(&run(relations_sql(catalog, schema)).await?)?;
            if relations.is_empty() {
                // A missing schema fails here; a real empty one answers
                // nothing, as `information_schema` did.
                let probe = run(schema_probe_sql(catalog, schema)).await?;
                if !probe.rows.is_empty() {
                    return Err(format!(
                        "information_schema listed no table in a schema where SHOW TABLES \
                         lists {}",
                        probe.rows.len()
                    ));
                }
            }
            Ok(CatalogLevel::Relations(relations))
        }
        (Some(schema), Some(table)) => Ok(CatalogLevel::Columns(parse_columns(
            &run(columns_sql(catalog, schema, table)).await?,
        )?)),
    }
}

/// Lists a catalog level on one Trino engine as `egret-lineage` (module doc).
pub struct TrinoCatalogLister {
    pool: sqlx::PgPool,
    minter: Arc<dyn LineageTokenMinter>,
    timeout: Duration,
}

impl TrinoCatalogLister {
    pub fn new(pool: sqlx::PgPool, minter: Arc<dyn LineageTokenMinter>, timeout: Duration) -> Self {
        Self {
            pool,
            minter,
            timeout,
        }
    }

    /// List one level of `catalog` on the Trino engine named `engine`: its
    /// schemas when `schema` is `None`, the schema's relations when `table`
    /// is `None`, the table's columns otherwise. `catalog`, `schema` and
    /// `table` are untrusted. `Err` is the reason, for the log; it never
    /// carries the token.
    pub async fn list(
        &self,
        engine: &str,
        catalog: &str,
        schema: Option<&str>,
        table: Option<&str>,
    ) -> Result<CatalogLevel, String> {
        if schema.is_none() && table.is_some() {
            return Err("a table needs a schema".to_string());
        }
        let spec = self.connection(engine, catalog).await?;
        let run = |sql: String| {
            let spec = spec.clone();
            async move {
                run_catalog_listing(&spec, &sql, MAX_LISTED, self.timeout)
                    .await
                    .map_err(|e| e.to_string())
            }
        };
        list_level(run, catalog, schema, table)
            .await
            .map_err(|e| format!("engine {engine}: {e}"))
    }

    /// The connection to `engine`, as `egret-lineage` with a fresh token, if
    /// it is one [`lineage_engines_via`] returns for `catalog`.
    async fn connection(&self, engine: &str, catalog: &str) -> Result<ConnSpec, String> {
        let candidates = {
            let mut conn = self
                .pool
                .acquire()
                .await
                .map_err(|e| format!("listing engine lookup: {e}"))?;
            lineage_engines_via(&mut conn, catalog)
                .await
                .map_err(|e| format!("listing engine lookup: {e}"))?
        };
        let Some(found) = candidates.into_iter().find(|e| e.name == engine) else {
            return Err(format!(
                "engine {engine} is not a running Trino engine that egret provisions, in JWT \
                 delegation mode, with this catalog attached and a delegated connection"
            ));
        };
        let options: ConnOptions = serde_json::from_value(found.options)
            .map_err(|e| format!("engine {engine}: unreadable connection options: {e}"))?;
        let token = self.minter.mint_lineage_token().await?;
        Ok(ConnSpec {
            engine: Engine::Trino,
            host: found.host,
            port: found.port,
            options,
            secret: None,
            session_user: Some(LINEAGE_PRINCIPAL.to_string()),
            bearer_token: Some(token),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egret_query_api::catalog_listing::ListedKind;
    use serde_json::json;

    /// A name with every character that could break out of a quoted form.
    const NASTY: &str = r#"Mi"x`e.d\"#;

    fn rows(rows: Vec<Vec<Value>>) -> TabularResult {
        TabularResult {
            columns: Vec::new(),
            row_count: rows.len() as u64,
            rows,
            bytes_scanned: None,
            truncated: false,
        }
    }

    /// A fake engine: answers each statement from `answers` by exact text,
    /// fails any other, and records what it was sent.
    struct Fake {
        answers: Vec<(String, Result<TabularResult, String>)>,
        sent: std::sync::Mutex<Vec<String>>,
    }

    impl Fake {
        fn new(answers: Vec<(String, Result<TabularResult, String>)>) -> Self {
            Self {
                answers,
                sent: Default::default(),
            }
        }

        async fn run(&self, sql: String) -> Result<TabularResult, String> {
            self.sent.lock().unwrap().push(sql.clone());
            self.answers
                .iter()
                .find(|(s, _)| *s == sql)
                .map(|(_, a)| a.clone())
                .unwrap_or_else(|| Err(format!("unexpected statement {sql}")))
        }

        fn sent(&self) -> Vec<String> {
            self.sent.lock().unwrap().clone()
        }
    }

    #[tokio::test]
    async fn a_schema_level_reads_information_schema_once() {
        let fake = Fake::new(vec![(
            relations_sql("ext", "sales"),
            Ok(rows(vec![vec![json!("orders"), json!("BASE TABLE")]])),
        )]);
        let got = list_level(|s| fake.run(s), "ext", Some("sales"), None)
            .await
            .unwrap();
        assert_eq!(
            got,
            CatalogLevel::Relations(vec![ListedRelation {
                name: "orders".into(),
                kind: ListedKind::Table
            }])
        );
        assert_eq!(fake.sent(), [relations_sql("ext", "sales")]);
    }

    #[tokio::test]
    async fn an_empty_schema_is_confirmed_before_it_is_listed_empty() {
        let fake = Fake::new(vec![
            (relations_sql("ext", "empty_s"), Ok(rows(vec![]))),
            (schema_probe_sql("ext", "empty_s"), Ok(rows(vec![]))),
        ]);
        let got = list_level(|s| fake.run(s), "ext", Some("empty_s"), None)
            .await
            .unwrap();
        assert_eq!(got, CatalogLevel::Relations(vec![]));
        assert_eq!(
            fake.sent(),
            [
                relations_sql("ext", "empty_s"),
                schema_probe_sql("ext", "empty_s")
            ]
        );
    }

    #[tokio::test]
    async fn a_missing_schema_is_an_error_not_an_empty_listing() {
        // information_schema answers nothing; SHOW TABLES fails (measured).
        let fake = Fake::new(vec![
            (relations_sql("ext", "nosuch"), Ok(rows(vec![]))),
            (
                schema_probe_sql("ext", "nosuch"),
                Err("engine: Schema 'nosuch' does not exist".into()),
            ),
        ]);
        let err = list_level(|s| fake.run(s), "ext", Some("nosuch"), None)
            .await
            .unwrap_err();
        assert!(err.contains("does not exist"), "{err}");
    }

    #[tokio::test]
    async fn tables_that_information_schema_misses_are_an_error() {
        let fake = Fake::new(vec![
            (relations_sql("ext", "s"), Ok(rows(vec![]))),
            (
                schema_probe_sql("ext", "s"),
                Ok(rows(vec![vec![json!("t")]])),
            ),
        ]);
        assert!(list_level(|s| fake.run(s), "ext", Some("s"), None)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn schemas_and_columns_are_one_statement_each() {
        let fake = Fake::new(vec![
            (
                schemas_sql("ext"),
                Ok(rows(vec![vec![json!("sales")], vec![json!("system")]])),
            ),
            (
                columns_sql("ext", "sales", "orders"),
                Ok(rows(vec![vec![json!("id"), json!("integer")]])),
            ),
        ]);
        assert_eq!(
            list_level(|s| fake.run(s), "ext", None, None).await,
            Ok(CatalogLevel::Schemas(vec!["sales".into()]))
        );
        assert_eq!(
            list_level(|s| fake.run(s), "ext", Some("sales"), Some("orders")).await,
            Ok(CatalogLevel::Columns(vec![ListedColumn {
                name: "id".into(),
                r#type: "integer".into()
            }]))
        );
        assert_eq!(
            fake.sent(),
            [schemas_sql("ext"), columns_sql("ext", "sales", "orders")]
        );
    }

    #[tokio::test]
    async fn a_table_without_a_schema_sends_nothing() {
        let fake = Fake::new(vec![]);
        assert!(list_level(|s| fake.run(s), "ext", None, Some("t"))
            .await
            .is_err());
        assert!(fake.sent().is_empty());
    }

    #[test]
    fn schemas_are_shown_from_the_quoted_catalog() {
        assert_eq!(schemas_sql("ext"), r#"SHOW SCHEMAS FROM "ext""#);
        assert_eq!(schemas_sql(NASTY), r#"SHOW SCHEMAS FROM "Mi""x`e.d\""#);
    }

    #[test]
    fn relations_read_information_schema_with_the_schema_as_a_literal() {
        assert_eq!(
            relations_sql("ext", "sales"),
            "SELECT table_name, table_type FROM \"ext\".information_schema.tables \
             WHERE table_schema = 'sales'"
        );
        // The literal doubles `'` and, on Trino, leaves a backslash alone.
        assert_eq!(
            relations_sql(NASTY, r"o'k\"),
            "SELECT table_name, table_type FROM \"Mi\"\"x`e.d\\\".information_schema.tables \
             WHERE table_schema = 'o''k\\'"
        );
    }

    #[test]
    fn the_schema_probe_and_describe_quote_every_part() {
        assert_eq!(
            schema_probe_sql("ext", NASTY),
            r#"SHOW TABLES FROM "ext"."Mi""x`e.d\""#
        );
        assert_eq!(
            columns_sql("ext", "sales", "orders"),
            r#"DESCRIBE "ext"."sales"."orders""#
        );
        assert_eq!(
            columns_sql(NASTY, "a.b", r#"t""#),
            r#"DESCRIBE "Mi""x`e.d\"."a.b"."t""""#
        );
    }

    #[test]
    fn schemas_are_sorted_without_the_hidden_ones() {
        // J1 and J8's rows together.
        let got = parse_schemas(&rows(vec![
            vec![json!("sales")],
            vec![json!("information_schema")],
            vec![json!("pg_catalog")],
            vec![json!("system")],
            vec![json!("empty_s")],
            vec![json!("Public")],
        ]))
        .unwrap();
        assert_eq!(got, ["Public", "empty_s", "pg_catalog", "sales"]);
    }

    #[test]
    fn relations_carry_their_kind_and_are_sorted() {
        let got = parse_relations(&rows(vec![
            vec![json!("v_t"), json!("VIEW")],
            vec![json!("t"), json!("BASE TABLE")],
            vec![json!("mv"), json!("MATERIALIZED VIEW")],
        ]))
        .unwrap();
        assert_eq!(
            got,
            [
                ListedRelation {
                    name: "mv".into(),
                    kind: ListedKind::MaterializedView
                },
                ListedRelation {
                    name: "t".into(),
                    kind: ListedKind::Table
                },
                ListedRelation {
                    name: "v_t".into(),
                    kind: ListedKind::View
                },
            ]
        );
    }

    #[test]
    fn a_relation_of_unknown_type_fails_the_listing() {
        let err = parse_relations(&rows(vec![vec![json!("t"), json!("SYNONYM")]])).unwrap_err();
        assert!(err.contains("SYNONYM"), "{err}");
    }

    #[test]
    fn columns_are_name_and_type_in_order() {
        // J4's answer.
        let got = parse_columns(&rows(vec![
            vec![json!("id"), json!("integer"), json!(""), json!("")],
            vec![
                json!("amount"),
                json!("decimal(10,2)"),
                json!(""),
                json!(""),
            ],
            vec![json!("Note"), json!("varchar"), json!(""), json!(null)],
        ]))
        .unwrap();
        assert_eq!(
            got,
            [
                ListedColumn {
                    name: "id".into(),
                    r#type: "integer".into()
                },
                ListedColumn {
                    name: "amount".into(),
                    r#type: "decimal(10,2)".into()
                },
                ListedColumn {
                    name: "Note".into(),
                    r#type: "varchar".into()
                },
            ]
        );
    }

    #[test]
    fn a_describe_with_no_column_is_an_error() {
        assert!(parse_columns(&rows(vec![])).is_err());
    }

    #[test]
    fn a_short_or_non_text_row_is_an_error() {
        assert!(parse_schemas(&rows(vec![vec![]])).is_err());
        assert!(parse_schemas(&rows(vec![vec![json!(null)]])).is_err());
        assert!(parse_relations(&rows(vec![vec![json!("t")]])).is_err());
        assert!(parse_relations(&rows(vec![vec![json!("t"), json!(1)]])).is_err());
        assert!(parse_columns(&rows(vec![vec![json!("id")]])).is_err());
        assert!(parse_columns(&rows(vec![vec![json!(null), json!("integer")]])).is_err());
    }

    #[test]
    fn a_truncated_answer_is_an_error_not_a_shorter_list() {
        let mut r = rows(vec![vec![json!("a")]]);
        r.truncated = true;
        assert!(parse_schemas(&r).is_err());
        let mut r = rows(vec![vec![json!("t"), json!("BASE TABLE")]]);
        r.truncated = true;
        assert!(parse_relations(&r).is_err());
        let mut r = rows(vec![vec![json!("id"), json!("integer")]]);
        r.truncated = true;
        assert!(parse_columns(&r).is_err());
    }
}
