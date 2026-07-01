//! `NotebookQueryRunner` — the engine-execution seam for the kernel query callback.
//! The concrete runner (egret-engine + egret-database) is injected from egret-bin,
//! so this crate stays free of an engine dependency.

use async_trait::async_trait;
use serde::Serialize;
use uuid::Uuid;

/// A result column (name + engine type label).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct QueryColumn {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

/// A fully-collected query result for the kernel callback.
#[derive(Debug, Clone, Serialize)]
pub struct QueryRows {
    pub columns: Vec<QueryColumn>,
    pub rows: Vec<Vec<serde_json::Value>>,
    pub truncated: bool,
}

/// Failure running a notebook query.
#[derive(Debug)]
pub enum QueryRunError {
    /// The database/engine could not be resolved.
    NotFound(String),
    /// The engine rejected the query (SQL error, timeout, connection failure).
    Engine(String),
}

#[async_trait]
pub trait NotebookQueryRunner: Send + Sync {
    /// Run `sql` against `database_id` as `session_user` (Trino `X-Trino-User`),
    /// presenting `bearer` as `Authorization: Bearer <token>` to the engine when
    /// provided (OIDC JWT from the user's stored refresh token), capped at `max_rows`.
    async fn run(
        &self,
        database_id: Uuid,
        session_user: String,
        bearer: Option<String>,
        sql: String,
        max_rows: usize,
    ) -> Result<QueryRows, QueryRunError>;
}

/// Test double: returns canned rows and records the last call.
#[cfg(any(test, feature = "test-util"))]
#[derive(Default)]
pub struct FakeQueryRunner {
    pub last: std::sync::Mutex<Option<(Uuid, String, String, usize)>>,
    /// Captures the `bearer` argument from the last `run` call for test assertions.
    pub last_bearer: std::sync::Mutex<Option<Option<String>>>,
}

#[cfg(any(test, feature = "test-util"))]
#[async_trait]
impl NotebookQueryRunner for FakeQueryRunner {
    async fn run(
        &self,
        database_id: Uuid,
        session_user: String,
        bearer: Option<String>,
        sql: String,
        max_rows: usize,
    ) -> Result<QueryRows, QueryRunError> {
        *self.last.lock().unwrap() = Some((database_id, session_user, sql, max_rows));
        *self.last_bearer.lock().unwrap() = Some(bearer);
        Ok(QueryRows {
            columns: vec![QueryColumn {
                name: "n".into(),
                ty: "integer".into(),
            }],
            rows: vec![vec![serde_json::json!(1)]],
            truncated: false,
        })
    }
}
