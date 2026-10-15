//! View governance B1: the production [`ExplainRunner`] — `EXPLAIN (TYPE IO,
//! FORMAT JSON)` of a view's SQL, run as `egret-lineage` on a Trino engine
//! bound to the view's catalog.
//!
//! # How the principal authenticates
//!
//! Through the same path as every delegated user query: an `aud: "engine"` JWT
//! signed with egret's machine-IdP key, presented as `Authorization: Bearer`
//! over the engine's TLS listener, with `preferred_username = egret-lineage`
//! (the engine's `principal-field`). Nothing is stored for it: no password, no
//! secret row. The only credential is egret's signing key, which only egret
//! holds; [`LineageTokenMinter`] is the seam, implemented in `egret-bin` beside
//! the per-user minter. No human can be issued the same name
//! (`egret_core::engine_principals`, migration 0187).
//!
//! # Which engine
//!
//! Any engine that is Trino, provisioned by egret (`origin = 'provisioned'`),
//! running, in JWT delegation mode, with the catalog attached and applied, and
//! with a materialized delegated connection (`query.databases`, which carries
//! the coordinator's TLS address and pinned certificate). An EXTERNAL engine is
//! never used: egret holds no credential for it and never connects to it
//! (`egret_engine_mgmt::external`), and its catalog names need not be the
//! registry's. Candidates are tried in name order; an unreachable one is
//! skipped, any other engine answer is final.

use std::sync::Arc;
use std::time::Duration;

use egret_perm_compile::lineage::{explain_only, io_plan_inputs, ExplainRunner, LINEAGE_PRINCIPAL};
use serde_json::Value;

use crate::execution::connector::{run_lineage_explain, ConnSpec};
use crate::execution::EngineError;
use egret_query_api::{ConnOptions, Engine};

/// Mints the engine JWT for `egret-lineage`. Takes no username on purpose: the
/// implementation can mint for this one principal and nothing else.
#[async_trait::async_trait]
pub trait LineageTokenMinter: Send + Sync {
    async fn mint_lineage_token(&self) -> Result<String, String>;
}

/// One engine the lineage EXPLAIN may run on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineageEngine {
    pub name: String,
    pub host: String,
    pub port: i32,
    pub options: Value,
}

/// The engines bound to `catalog` that the lineage EXPLAIN may use, by name
/// (module doc).
pub async fn lineage_engines_via(
    conn: &mut sqlx::PgConnection,
    catalog: &str,
) -> Result<Vec<LineageEngine>, sqlx::Error> {
    let rows: Vec<(String, String, i32, Value)> = sqlx::query_as(
        "SELECT e.name, d.host, d.port, d.options \
           FROM registry.catalogs c \
           JOIN registry.engine_catalog_attachments ea \
             ON ea.catalog_id = c.id AND ea.apply_status = 'applied' \
           JOIN engine_mgmt.engines e ON e.id = ea.engine_id \
           JOIN LATERAL ( \
                SELECT host, port, options FROM query.databases d \
                 WHERE d.engine_id = e.id AND d.delegated AND d.secret_id IS NULL \
                   AND d.engine = 'trino' \
                 ORDER BY d.id LIMIT 1) d ON true \
          WHERE c.name = $1 AND e.type = 'trino' AND e.origin = 'provisioned' \
            AND e.status = 'running' \
            AND e.spec->'trinoConfig'->>'delegationMode' = 'jwt' \
          ORDER BY e.name",
    )
    .bind(catalog)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(name, host, port, options)| LineageEngine {
            name,
            host,
            port,
            options,
        })
        .collect())
}

/// The plan JSON out of an EXPLAIN's one-row, one-column answer.
pub(crate) fn plan_of(result: &egret_core::TabularResult) -> Result<Value, String> {
    if result.truncated || result.rows.len() != 1 || result.rows[0].len() != 1 {
        return Err(format!(
            "the engine answered the EXPLAIN with {} rows, not one plan",
            result.rows.len()
        ));
    }
    let plan = match &result.rows[0][0] {
        Value::String(text) => {
            serde_json::from_str(text).map_err(|e| format!("the engine's plan is not JSON: {e}"))?
        }
        other => other.clone(),
    };
    io_plan_inputs(&plan)?;
    Ok(plan)
}

/// The production [`ExplainRunner`] (module doc).
pub struct TrinoLineageRunner {
    pool: sqlx::PgPool,
    minter: Arc<dyn LineageTokenMinter>,
    timeout: Duration,
}

impl TrinoLineageRunner {
    pub fn new(pool: sqlx::PgPool, minter: Arc<dyn LineageTokenMinter>, timeout: Duration) -> Self {
        Self {
            pool,
            minter,
            timeout,
        }
    }
}

#[async_trait::async_trait]
impl ExplainRunner for TrinoLineageRunner {
    async fn explain_io(
        &self,
        default_catalog: &str,
        default_schema: &str,
        sql: &str,
    ) -> Result<Value, String> {
        let statement = explain_only(sql)?;
        let engines = {
            let mut conn = self
                .pool
                .acquire()
                .await
                .map_err(|e| format!("lineage engine lookup: {e}"))?;
            lineage_engines_via(&mut conn, default_catalog)
                .await
                .map_err(|e| format!("lineage engine lookup: {e}"))?
        };
        if engines.is_empty() {
            return Err(format!(
                "no running Trino engine that egret provisions, in JWT delegation mode and \
                 assigned to a workspace, has catalog {default_catalog:?} attached"
            ));
        }
        let token = self.minter.mint_lineage_token().await?;
        let mut unreachable = Vec::new();
        for engine in engines {
            let options: ConnOptions =
                serde_json::from_value(engine.options.clone()).map_err(|e| {
                    format!("engine {}: unreadable connection options: {e}", engine.name)
                })?;
            let spec = ConnSpec {
                engine: Engine::Trino,
                host: engine.host.clone(),
                port: engine.port,
                options,
                secret: None,
                session_user: Some(LINEAGE_PRINCIPAL.to_string()),
                bearer_token: Some(token.clone()),
            };
            match run_lineage_explain(
                &spec,
                default_catalog,
                default_schema,
                &statement,
                self.timeout,
            )
            .await
            {
                Ok(result) => {
                    return plan_of(&result).map_err(|e| format!("engine {}: {e}", engine.name))
                }
                Err(EngineError::Connect(e)) => unreachable.push(format!("{}: {e}", engine.name)),
                Err(e) => return Err(format!("engine {}: {e}", engine.name)),
            }
        }
        Err(format!(
            "no Trino engine bound to catalog {default_catalog:?} was reachable: {}",
            unreachable.join("; ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egret_core::TabularResult;
    use serde_json::json;

    fn one(v: Value) -> TabularResult {
        TabularResult {
            columns: Vec::new(),
            rows: vec![vec![v]],
            row_count: 1,
            bytes_scanned: None,
            truncated: false,
        }
    }

    #[test]
    fn the_plan_is_read_from_its_one_cell() {
        let plan = plan_of(&one(json!(
            "{\"inputTableColumnInfos\":[],\"estimate\":{}}"
        )))
        .unwrap();
        assert_eq!(plan["inputTableColumnInfos"], json!([]));
    }

    #[test]
    fn an_answer_that_is_not_the_io_plan_is_refused() {
        for cell in [
            json!("not json"),
            json!("{\"estimate\":{}}"),
            json!("[]"),
            json!(1),
        ] {
            assert!(plan_of(&one(cell.clone())).is_err(), "{cell}");
        }
        let mut two = one(json!("{\"inputTableColumnInfos\":[]}"));
        two.rows.push(vec![json!("x")]);
        assert!(plan_of(&two).is_err());
    }
}
