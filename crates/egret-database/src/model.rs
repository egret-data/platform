//! Domain types: engine enum, connection options, DB row, request/response DTOs.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Supported query engines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Trino,
    StarRocks,
}

impl Engine {
    pub fn as_str(self) -> &'static str {
        match self {
            Engine::Trino => "trino",
            Engine::StarRocks => "starrocks",
        }
    }

    pub fn parse(s: &str) -> Option<Engine> {
        match s {
            "trino" => Some(Engine::Trino),
            "starrocks" => Some(Engine::StarRocks),
            _ => None,
        }
    }
}

/// Non-secret connection options stored in `databases.options` (jsonb).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConnOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Trino only: "http" | "https".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_scheme: Option<String>,
    /// Trino default catalog (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<String>,
    /// StarRocks default database (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
}

/// A full database registration row (no secret material).
#[derive(Debug, Clone)]
pub struct Database {
    pub id: Uuid,
    pub name: String,
    pub engine: Engine,
    pub host: String,
    pub port: i32,
    pub options: ConnOptions,
    pub has_secret: bool,
    pub status: String,
}

// ── Request DTOs ──────────────────────────────────────────────────────────────

/// Register/update body. Credentials are a NAMED secret (Epic B): either
/// `new_secret` (mint a new named secret inline) or `secret_id` (reference an
/// existing named secret in the workspace). Omitting both registers a connection
/// with no secret (or, on update, keeps the existing one).
#[derive(Debug, Deserialize)]
pub struct DatabaseInput {
    pub name: String,
    pub engine: Engine,
    pub host: String,
    pub port: i32,
    #[serde(default)]
    pub options: ConnOptions,
    #[serde(default)]
    pub secret_id: Option<Uuid>,
    #[serde(default)]
    pub new_secret: Option<egret_secrets::NewSecret>,
}

/// Body for testing an unsaved connection (no `name`).
#[derive(Debug, Deserialize)]
pub struct TestInput {
    pub engine: Engine,
    pub host: String,
    pub port: i32,
    #[serde(default)]
    pub options: ConnOptions,
    #[serde(default)]
    pub secret: Option<String>,
}

// ── Response DTOs ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct DatabaseDetailDto {
    pub id: String,
    pub name: String,
    pub engine: Engine,
    pub host: String,
    pub port: i32,
    pub options: ConnOptions,
    pub has_secret: bool,
    pub status: String,
}

impl From<Database> for DatabaseDetailDto {
    fn from(d: Database) -> Self {
        DatabaseDetailDto {
            id: d.id.to_string(),
            name: d.name,
            engine: d.engine,
            host: d.host,
            port: d.port,
            options: d.options,
            has_secret: d.has_secret,
            status: d.status,
        }
    }
}

/// Minimal listing for the Explorer DB picker (no connection internals).
#[derive(Debug, Serialize)]
pub struct DatabaseListDto {
    pub id: String,
    pub name: String,
    pub engine: Engine,
}

/// Result of a connection test.
#[derive(Debug, Clone, Serialize)]
pub struct TestOutcome {
    pub ok: bool,
    pub latency_ms: u64,
    pub detail: String,
}
