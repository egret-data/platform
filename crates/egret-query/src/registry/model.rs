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
    /// B3a: run BI queries as the caller's per-user engine JWT (delegation).
    #[serde(default)]
    pub delegated: bool,
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
    pub delegated: bool,
    /// Set (as a UUID string) when this connection was materialized by an
    /// engine→workspace assignment (read-only, provisioned by egret); `None`
    /// for a self-service connection. The frontend uses it to split the
    /// workspace Engines page into provisioned vs. self-service groups.
    pub engine_id: Option<String>,
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
            delegated: d.delegated,
            engine_id: d.engine_id.map(|u| u.to_string()),
        }
    }
}

/// Minimal listing for the Explorer DB picker (no connection internals).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DatabaseListDto {
    pub id: String,
    pub name: String,
    pub engine: Engine,
    pub delegated: bool,
}

// ── B2: ws-member read-only view of assigned engines ──────────────────────────

/// One egret-provisioned engine assigned to the workspace (list view).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct WsEngineListDto {
    pub id: String,
    pub name: String,
    /// Engine kind (`trino`/`starrocks`/`risingwave`).
    #[serde(rename = "type")]
    pub engine_type: String,
    pub status: String,
}

/// One catalog of an assigned engine, with the storage/secret NAME it maps to.
/// `storage_name`/`secret_name` are best-effort (`null` when unresolvable); the
/// secret VALUE is never exposed.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct WsEngineCatalogDto {
    pub name: String,
    pub kind: String,
    pub storage_name: Option<String>,
    pub secret_name: Option<String>,
}

/// Read-only detail of an assigned engine: its catalogs, each resolved to the
/// storage connection (via the engine's warehouse) and secret name it uses.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct WsEngineDetailDto {
    pub name: String,
    /// Engine kind (`trino`/`starrocks`/`risingwave`).
    #[serde(rename = "type")]
    pub engine_type: String,
    pub status: String,
    pub catalogs: Vec<WsEngineCatalogDto>,
}

/// Result of a connection test.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct TestOutcome {
    pub ok: bool,
    pub latency_ms: u64,
    pub detail: String,
}
