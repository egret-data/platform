//! StarRocks connector (implemented in Task 4).
use super::{ConnSpec, Connector};
use crate::error::EngineError;
use crate::model::{Limits, QueryResult};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

pub struct StarRocksConnector;
impl StarRocksConnector {
    pub fn new(_spec: &ConnSpec) -> Self {
        StarRocksConnector
    }
}
#[async_trait]
impl Connector for StarRocksConnector {
    async fn execute(
        &self,
        _sql: &str,
        _limits: &Limits,
        _cancel: CancellationToken,
    ) -> Result<QueryResult, EngineError> {
        Err(EngineError::Engine(
            "starrocks connector not yet implemented".into(),
        ))
    }
}
