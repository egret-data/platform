//! `egret-query-api` — leaf contract crate for the Query seam (E4d).
//!
//! Holds the shared domain vocabulary — [`Engine`], [`ConnOptions`],
//! [`Database`] — that both `egret-query` (the seam) and its cross-seam
//! consumers (e.g. `egret-bi`) need, without forcing consumers onto deep
//! `egret_query::registry::model::…` paths or pulling in the seam's sqlx/axum
//! machinery. Also re-exports the core tabular result type so the seam's result
//! shape is reachable via the contract crate too.
//!
//! Pure-serde DTOs only: no sqlx `FromRow`/`Type`, no axum, no `QueryPort` trait
//! (the seam's runtime surface is the concrete `Connector` data-plane).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The unified tabular query result + column metadata, re-exported from
/// `egret-core` so consumers can reach the Query seam's result shape through the
/// contract crate.
pub use egret_core::{ColumnMeta, TabularResult};

/// Supported query engines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
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
    /// Trino only: run each query as the logged-in user (X-Trino-User).
    /// Requires Trino impersonation + Ranger. Default false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impersonate: Option<bool>,
}

/// A full database registration row (no secret material).
///
/// `Serialize`/`Deserialize` are forward-looking contract surface: today the
/// wire type is the separate `DatabaseDetailDto` and the store builds `Database`
/// field-by-field from sqlx rows, so these codecs are not yet exercised — they
/// let a future out-of-process query-seam client exchange this domain type.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Database {
    pub id: Uuid,
    pub name: String,
    pub engine: Engine,
    pub host: String,
    pub port: i32,
    pub options: ConnOptions,
    pub has_secret: bool,
    pub status: String,
    /// B3a: run BI queries as the caller's per-user engine JWT (delegation).
    #[serde(default)]
    pub delegated: bool,
    /// Set when this row was materialized by an engine→workspace assignment.
    /// A non-null `engine_id` marks the connection provisioned (read-only,
    /// per-user-delegated) and links it back to the source engine.
    #[serde(default)]
    pub engine_id: Option<Uuid>,
}
