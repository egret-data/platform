//! Per-engine schema introspection via the engine connectors (synchronous, small queries).

use std::time::Duration;

use egret_query::execution::connector::{build_connector, ConnSpec};
use egret_query::execution::model::Limits;
use egret_query_api::Engine;
use tokio_util::sync::CancellationToken;

use crate::error::BiError;
use crate::model::{SchemaColumn, SearchHit};

#[derive(Clone, Default)]
pub struct SchemaService;

impl SchemaService {
    pub fn new() -> Self {
        SchemaService
    }

    fn limits() -> Limits {
        Limits {
            max_rows: 10_000,
            timeout: Duration::from_secs(15),
        }
    }

    /// Run a meta-query and return the first column of every row as a String.
    async fn first_col(&self, spec: &ConnSpec, sql: &str) -> Result<Vec<String>, BiError> {
        let conn = build_connector(spec);
        let r = conn
            .execute(sql, &Self::limits(), CancellationToken::new())
            .await?;
        Ok(r.rows
            .into_iter()
            .filter_map(|row| row.into_iter().next())
            .map(val_to_string)
            .collect())
    }

    pub async fn catalogs(&self, spec: &ConnSpec) -> Result<Vec<String>, BiError> {
        match spec.engine {
            Engine::Trino => self.first_col(spec, "SHOW CATALOGS").await,
            Engine::StarRocks => {
                let conn = build_connector(spec);
                let r = conn
                    .execute("SHOW CATALOGS", &Self::limits(), CancellationToken::new())
                    .await?;
                Ok(starrocks_served_catalogs(r.rows))
            }
        }
    }

    pub async fn schemas(&self, spec: &ConnSpec, catalog: &str) -> Result<Vec<String>, BiError> {
        self.first_col(spec, &schemas_sql(spec.engine, catalog))
            .await
    }

    pub async fn tables(
        &self,
        spec: &ConnSpec,
        catalog: &str,
        schema: &str,
    ) -> Result<Vec<String>, BiError> {
        self.first_col(spec, &tables_sql(spec.engine, catalog, schema))
            .await
    }

    /// Global object search: find tables/views whose name matches `query`
    /// anywhere in the database. Trino searches across catalogs via the JDBC
    /// metadata table; StarRocks uses information_schema (no catalog level).
    pub async fn search(&self, spec: &ConnSpec, query: &str) -> Result<Vec<SearchHit>, BiError> {
        let sql = search_sql(spec.engine, query);
        let conn = build_connector(spec);
        let r = conn
            .execute(&sql, &Self::limits(), CancellationToken::new())
            .await?;
        Ok(r.rows
            .into_iter()
            .map(|row| {
                let mut it = row.into_iter();
                SearchHit {
                    catalog: it.next().map(val_to_string).unwrap_or_default(),
                    schema: it.next().map(val_to_string).unwrap_or_default(),
                    table: it.next().map(val_to_string).unwrap_or_default(),
                }
            })
            .collect())
    }

    pub async fn columns(
        &self,
        spec: &ConnSpec,
        catalog: &str,
        schema: &str,
        table: &str,
    ) -> Result<Vec<SchemaColumn>, BiError> {
        let conn = build_connector(spec);
        let sql = columns_sql(spec.engine, catalog, schema, table);
        let r = conn
            .execute(&sql, &Self::limits(), CancellationToken::new())
            .await?;
        Ok(r.rows
            .into_iter()
            .map(|row| {
                let mut it = row.into_iter();
                SchemaColumn {
                    name: it.next().map(val_to_string).unwrap_or_default(),
                    ty: it.next().map(val_to_string).unwrap_or_default(),
                }
            })
            .collect())
    }
}

const STARROCKS_INTERNAL_CATALOG_TYPE: &str = "Internal";

/// The catalogs a StarRocks connection offers in the explorer: the external
/// catalogs the engine serves (`SHOW CATALOGS` rows are `Catalog, Type,
/// Comment`). The built-in `Internal` catalog (`default_catalog`, whose
/// databases are `_statistics_`, `information_schema` and `sys`) holds nothing
/// egret serves, so it is left out (KI-338).
fn starrocks_served_catalogs(rows: Vec<Vec<serde_json::Value>>) -> Vec<String> {
    rows.into_iter()
        .filter_map(|row| {
            let mut it = row.into_iter();
            let name = it.next().map(val_to_string)?;
            let ty = it.next().map(val_to_string).unwrap_or_default();
            let internal = if ty.is_empty() {
                name == "default_catalog"
            } else {
                ty.eq_ignore_ascii_case(STARROCKS_INTERNAL_CATALOG_TYPE)
            };
            (!internal).then_some(name)
        })
        .collect()
}

fn schemas_sql(engine: Engine, catalog: &str) -> String {
    match engine {
        Engine::Trino => format!("SHOW SCHEMAS FROM {}", quote_trino(catalog)),
        Engine::StarRocks if catalog.is_empty() => "SHOW DATABASES".to_string(),
        Engine::StarRocks => format!("SHOW DATABASES FROM {}", quote_mysql(catalog)),
    }
}

fn tables_sql(engine: Engine, catalog: &str, schema: &str) -> String {
    match engine {
        Engine::Trino => format!(
            "SHOW TABLES FROM {}.{}",
            quote_trino(catalog),
            quote_trino(schema)
        ),
        Engine::StarRocks if catalog.is_empty() => {
            format!("SHOW TABLES FROM {}", quote_mysql(schema))
        }
        Engine::StarRocks => format!(
            "SHOW TABLES FROM {}.{}",
            quote_mysql(catalog),
            quote_mysql(schema)
        ),
    }
}

fn columns_sql(engine: Engine, catalog: &str, schema: &str, table: &str) -> String {
    match engine {
        Engine::Trino => format!(
            "DESCRIBE {}.{}.{}",
            quote_trino(catalog),
            quote_trino(schema),
            quote_trino(table)
        ),
        Engine::StarRocks if catalog.is_empty() => {
            format!("DESCRIBE {}.{}", quote_mysql(schema), quote_mysql(table))
        }
        Engine::StarRocks => format!(
            "DESCRIBE {}.{}.{}",
            quote_mysql(catalog),
            quote_mysql(schema),
            quote_mysql(table)
        ),
    }
}

/// The object-search statement. Same-named tables are ordered by catalog and
/// schema after the name (KI-160): ordered by the name alone, the engine
/// returned `customer` from `tpch.sf1000`, `tpch.sf300`, …, `tpch.tiny` in a
/// different order on each run, and which 200 rows the `LIMIT` kept was not
/// fixed either.
fn search_sql(engine: Engine, query: &str) -> String {
    let like = like_literal(query);
    match engine {
        Engine::Trino => format!(
            "SELECT table_cat, table_schem, table_name FROM system.jdbc.tables \
             WHERE lower(table_name) LIKE lower('%{like}%') ESCAPE '\\' \
             ORDER BY table_name, table_cat, table_schem LIMIT 200"
        ),
        Engine::StarRocks => format!(
            "SELECT '', table_schema, table_name FROM information_schema.tables \
             WHERE lower(table_name) LIKE lower('%{like}%') ESCAPE '\\\\' \
             ORDER BY table_name, table_schema LIMIT 200"
        ),
    }
}

fn val_to_string(v: serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s,
        other => other.to_string(),
    }
}

/// Escape a user search term for a `LIKE` pattern used inside a single-quoted
/// SQL string with `ESCAPE '\'`: double the string quote, and backslash-escape
/// the LIKE wildcards so the term matches literally (no injection, no wildcard
/// surprises).
fn like_literal(q: &str) -> String {
    let mut out = String::with_capacity(q.len() + 4);
    for c in q.chars() {
        match c {
            '\'' => out.push_str("''"),
            '\\' | '%' | '_' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// Quote a Trino identifier: wrap in double-quotes, escape internal quotes.
pub(crate) fn quote_trino(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}
/// Quote a MySQL/StarRocks identifier: backticks, escape internal backticks.
pub(crate) fn quote_mysql(ident: &str) -> String {
    format!("`{}`", ident.replace('`', "``"))
}

/// Quote an identifier for the given engine.
pub(crate) fn quote_ident(engine: Engine, ident: &str) -> String {
    match engine {
        Engine::Trino => quote_trino(ident),
        Engine::StarRocks => quote_mysql(ident),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        columns_sql, like_literal, schemas_sql, search_sql, starrocks_served_catalogs, tables_sql,
    };
    use egret_query_api::Engine;

    /// KI-160: same-named tables come back in a fixed order, by catalog and
    /// schema after the name, on both engines.
    #[test]
    fn search_orders_same_named_tables_by_catalog_and_schema() {
        assert!(search_sql(Engine::Trino, "customer")
            .contains("ORDER BY table_name, table_cat, table_schem LIMIT 200"));
        assert!(search_sql(Engine::StarRocks, "customer")
            .contains("ORDER BY table_name, table_schema LIMIT 200"));
    }

    #[test]
    fn like_literal_neutralizes_quotes_and_wildcards() {
        // Single quotes are doubled so a term can't break out of the string literal.
        assert_eq!(like_literal("o'brien"), "o''brien");
        // LIKE wildcards + the escape char are backslash-escaped → matched literally.
        assert_eq!(like_literal("50%_x"), "50\\%\\_x");
        assert_eq!(like_literal("a\\b"), "a\\\\b");
        // A plain term passes through unchanged.
        assert_eq!(like_literal("orders"), "orders");
        // An injection attempt stays inert (quotes doubled, no early terminator).
        assert_eq!(like_literal("'; DROP TABLE t --"), "''; DROP TABLE t --");
    }

    fn row(cols: &[&str]) -> Vec<serde_json::Value> {
        cols.iter().map(|c| serde_json::json!(c)).collect()
    }

    /// KI-338: the explorer offers the catalog the engine serves, not
    /// StarRocks' built-in one.
    #[test]
    fn starrocks_explorer_lists_the_served_catalog_not_default_catalog() {
        let rows = vec![
            row(&["default_catalog", "Internal", "An internal catalog"]),
            row(&["egret", "Iceberg", ""]),
        ];
        assert_eq!(starrocks_served_catalogs(rows), vec!["egret"]);
        // No Type column: fall back on the built-in's fixed name.
        let rows = vec![row(&["default_catalog"]), row(&["egret"])];
        assert_eq!(starrocks_served_catalogs(rows), vec!["egret"]);
    }

    /// KI-338: with a catalog picked, schemas, tables and columns are read
    /// from that catalog, not from `default_catalog`.
    #[test]
    fn starrocks_listings_are_scoped_to_the_picked_catalog() {
        let sr = Engine::StarRocks;
        assert_eq!(schemas_sql(sr, "egret"), "SHOW DATABASES FROM `egret`");
        assert_eq!(
            tables_sql(sr, "egret", "sales"),
            "SHOW TABLES FROM `egret`.`sales`"
        );
        assert_eq!(
            columns_sql(sr, "egret", "sales", "orders"),
            "DESCRIBE `egret`.`sales`.`orders`"
        );
        // A client that sends no catalog keeps the old, session-scoped listing.
        assert_eq!(schemas_sql(sr, ""), "SHOW DATABASES");
        assert_eq!(tables_sql(sr, "", "sales"), "SHOW TABLES FROM `sales`");
    }
}
