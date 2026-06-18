//! Connector trait + per-engine construction + value mapping.

use async_trait::async_trait;
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
    async fn execute(
        &self,
        sql: &str,
        limits: &Limits,
        cancel: CancellationToken,
    ) -> Result<QueryResult, EngineError>;
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
