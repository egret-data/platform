//! Per-engine schema introspection via the engine connectors (synchronous, small queries).

use std::time::Duration;

use egret_database::model::Engine;
use egret_engine::connector::{build_connector, ConnSpec};
use egret_engine::model::Limits;
use tokio_util::sync::CancellationToken;

use crate::error::BiError;
use crate::model::SchemaColumn;

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
            Engine::StarRocks => Ok(vec![]), // no catalog level
        }
    }

    pub async fn schemas(&self, spec: &ConnSpec, catalog: &str) -> Result<Vec<String>, BiError> {
        match spec.engine {
            Engine::Trino => {
                self.first_col(spec, &format!("SHOW SCHEMAS FROM {}", quote_trino(catalog)))
                    .await
            }
            Engine::StarRocks => self.first_col(spec, "SHOW DATABASES").await,
        }
    }

    pub async fn tables(
        &self,
        spec: &ConnSpec,
        catalog: &str,
        schema: &str,
    ) -> Result<Vec<String>, BiError> {
        match spec.engine {
            Engine::Trino => {
                self.first_col(
                    spec,
                    &format!(
                        "SHOW TABLES FROM {}.{}",
                        quote_trino(catalog),
                        quote_trino(schema)
                    ),
                )
                .await
            }
            Engine::StarRocks => {
                self.first_col(spec, &format!("SHOW TABLES FROM {}", quote_mysql(schema)))
                    .await
            }
        }
    }

    pub async fn columns(
        &self,
        spec: &ConnSpec,
        catalog: &str,
        schema: &str,
        table: &str,
    ) -> Result<Vec<SchemaColumn>, BiError> {
        let conn = build_connector(spec);
        let sql = match spec.engine {
            Engine::Trino => format!(
                "DESCRIBE {}.{}.{}",
                quote_trino(catalog),
                quote_trino(schema),
                quote_trino(table)
            ),
            Engine::StarRocks => format!("DESCRIBE {}.{}", quote_mysql(schema), quote_mysql(table)),
        };
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

fn val_to_string(v: serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s,
        other => other.to_string(),
    }
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
