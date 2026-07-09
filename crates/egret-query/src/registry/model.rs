//! Registry request/response DTOs, plus the shared domain vocabulary
//! (engine enum, connection options, DB row) re-exported from `egret-query-api`.
//!
//! The shared domain vocabulary — [`Engine`], [`ConnOptions`], [`Database`] —
//! lives in the leaf `egret-query-api` contract crate (E4d) so cross-seam
//! consumers depend on it without reaching through this deep path. It's
//! re-exported here so `egret_query::registry::model::…` and internal
//! `crate::registry::model::…` call sites keep resolving unchanged.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use egret_query_api::{ConnOptions, Database, Engine};

// ── Request DTOs ──────────────────────────────────────────────────────────────

/// Register/update body. Credentials are a NAMED secret (Epic B): either
/// `new_secret` (mint a new named secret inline) or `secret_id` (reference an
/// existing named secret in the workspace). Omitting both registers a connection
/// with no secret (or, on update, keeps the existing one).
#[derive(Debug, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Deserialize, utoipa::ToSchema)]
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

#[derive(Debug, Serialize, utoipa::ToSchema)]
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
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DatabaseListDto {
    pub id: String,
    pub name: String,
    pub engine: Engine,
}

/// Result of a connection test.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct TestOutcome {
    pub ok: bool,
    pub latency_ms: u64,
    pub detail: String,
}
