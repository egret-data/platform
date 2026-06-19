//! Connector trait + per-engine construction + value mapping.

use async_trait::async_trait;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use egret_database::model::{ConnOptions, Database, Engine};

use crate::error::EngineError;
use crate::model::{Limits, QueryResult};

mod starrocks;
mod trino;

/// Everything needed to open a connection for one query.
#[derive(Debug, Clone)]
pub struct ConnSpec {
    pub engine: Engine,
    pub host: String,
    pub port: i32,
    pub options: ConnOptions,
    pub secret: Option<String>,
}

impl ConnSpec {
    /// Build from a resolved registry `Database` + its opened secret.
    pub fn from_database(db: &Database, secret: Option<String>) -> Self {
        ConnSpec {
            engine: db.engine,
            host: db.host.clone(),
            port: db.port,
            options: db.options.clone(),
            secret,
        }
    }
}

#[async_trait]
pub trait Connector: Send + Sync {
    /// Run `sql`, honoring `limits` and `cancel`. Fetches up to `max_rows + 1` rows;
    /// if the extra row appears, sets `truncated` and drops it. On a triggered
    /// `cancel`, best-effort aborts the engine query and returns `EngineError::Cancelled`.
    ///
    /// Cancellation is observed cooperatively between fetched pages/rows; a query
    /// blocked in a single network read is interrupted by the JobManager's outer
    /// timeout, which drops the future.
    async fn execute(
        &self,
        sql: &str,
        limits: &Limits,
        cancel: CancellationToken,
    ) -> Result<QueryResult, EngineError>;

    /// Return the typed columns of an arbitrary `SELECT` without fetching rows.
    /// Default: wrap the query as `LIMIT 0` and reuse `execute`'s column metadata —
    /// correct for engines whose `execute` already yields clean type names (Trino).
    /// Engines whose `execute` emits protocol-level type tags override this.
    async fn describe(
        &self,
        sql: &str,
        timeout: Duration,
    ) -> Result<Vec<crate::model::Column>, EngineError> {
        let wrapped = format!("SELECT * FROM (\n{sql}\n) AS _egret_ds LIMIT 0");
        let limits = Limits {
            max_rows: 0,
            timeout,
        };
        let r = self
            .execute(&wrapped, &limits, CancellationToken::new())
            .await?;
        Ok(r.columns)
    }
}

/// Construct the right connector for a spec.
pub fn build_connector(spec: &ConnSpec) -> Box<dyn Connector> {
    match spec.engine {
        Engine::Trino => Box::new(trino::TrinoConnector::new(spec)),
        Engine::StarRocks => Box::new(starrocks::StarRocksConnector::new(spec)),
    }
}

/// A production `ConnectorFactory` resolves a database id → connector via the registry.
/// Defined here so `JobManager` can be tested with a fake factory.
#[async_trait]
pub trait ConnectorFactory: Send + Sync {
    async fn build(
        &self,
        db: &Database,
        secret: Option<String>,
    ) -> Result<Box<dyn Connector>, EngineError>;
}

/// The default factory: build directly from the registry row.
pub struct DefaultConnectorFactory;

#[async_trait]
impl ConnectorFactory for DefaultConnectorFactory {
    async fn build(
        &self,
        db: &Database,
        secret: Option<String>,
    ) -> Result<Box<dyn Connector>, EngineError> {
        Ok(build_connector(&ConnSpec::from_database(db, secret)))
    }
}
