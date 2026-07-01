//! The engine-connection seam: resolve a database_id to the coordinates a kernel
//! needs to open a direct Daft connection. Concrete impl injected from egret-bin
//! (keeps this crate free of an egret-database dependency).

use async_trait::async_trait;
use uuid::Uuid;

/// Connection coordinates for a governed engine.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct EngineConn {
    /// "trino" | "starrocks".
    pub kind: String,
    /// Engine host.
    pub host: String,
    /// Engine port.
    pub port: i32,
}

#[derive(Debug)]
pub enum EngineConnError {
    /// The database/engine could not be resolved.
    NotFound(String),
    /// An unexpected error resolving the connection.
    Other(String),
}

#[async_trait]
pub trait EngineConnResolver: Send + Sync {
    /// Resolve `database_id` to its connection coordinates.
    async fn resolve(&self, database_id: Uuid) -> Result<EngineConn, EngineConnError>;
}

/// Test double: returns the configured conn, or `NotFound` when unset.
#[cfg(any(test, feature = "test-util"))]
#[derive(Default)]
pub struct FakeEngineConnResolver {
    pub conn: Option<EngineConn>,
}

#[cfg(any(test, feature = "test-util"))]
#[async_trait]
impl EngineConnResolver for FakeEngineConnResolver {
    async fn resolve(&self, _database_id: Uuid) -> Result<EngineConn, EngineConnError> {
        self.conn
            .clone()
            .ok_or_else(|| EngineConnError::NotFound("no fake conn".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fake_resolver_returns_configured_conn() {
        let r = FakeEngineConnResolver {
            conn: Some(EngineConn {
                kind: "trino".into(),
                host: "trino".into(),
                port: 8080,
            }),
        };
        let got = r.resolve(Uuid::nil()).await.unwrap();
        assert_eq!(
            got,
            EngineConn {
                kind: "trino".into(),
                host: "trino".into(),
                port: 8080
            }
        );
    }

    #[tokio::test]
    async fn fake_resolver_defaults_to_not_found() {
        let r = FakeEngineConnResolver::default();
        let err = r.resolve(Uuid::nil()).await.unwrap_err();
        assert!(matches!(err, EngineConnError::NotFound(_)));
    }
}
