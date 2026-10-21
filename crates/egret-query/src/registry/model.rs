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
    /// The data behind this connection is governed at the engine (Ranger row
    /// filters / column masks). Publishing a Data API endpoint onto it — or
    /// previewing one against it — requires `delegated` or
    /// `options.impersonate`; see [`Database::governed`].
    ///
    /// # Why this one field is `Option` on a body where nothing else is
    ///
    /// `PUT /ws/{ws}/databases/{id}` is a WHOLE-ROW update: every field of this
    /// body is written, so an omitted field is written as its `serde` default.
    /// For `name` or `port` that is merely an API a client must send in full.
    /// For a security flag it is a control that a client which predates the
    /// field — a script, an IaC provider, a `curl` recipe copied before this
    /// shipped — **silently turns off by editing a port**, with a 200, nothing
    /// logged, and the endpoints on that connection quietly un-gated. That is
    /// the same failure class the flag exists to prevent: a protection that
    /// depends on every writer remembering it.
    ///
    /// So `None` means "leave it as it is" and only an explicit `false` clears
    /// it. Absence cannot clear the control; clearing is a thing a client says.
    /// The keep-current semantics live in the SQL (`COALESCE($n, governed)` in
    /// [`crate::registry::repo::DatabaseRepo::update_in_workspace`]) rather than
    /// in a read-modify-write here, so there is no window between reading the
    /// old value and writing it back.
    ///
    /// On CREATE there is nothing to keep, and `None` is `false` — the same
    /// direction migration 0130 defaulted the column to, for the reason stated
    /// there: it is the value that changes nothing, not the value that is
    /// safest.
    #[serde(default)]
    pub governed: Option<bool>,
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
    /// See [`Database::governed`]. Surfaced on the detail DTO so the Data API
    /// author's database picker can warn BEFORE publish that the connection it
    /// is pointed at cannot carry a governed endpoint.
    pub governed: bool,
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
            governed: d.governed,
            engine_id: d.engine_id.map(|u| u.to_string()),
        }
    }
}

/// A connection as `GET /ws/{ws}/databases` shows it to a member who is not a
/// workspace admin but may run queries or read datasets (KI-159, decision D9):
/// enough to pick it in SQL Lab, nothing about how to reach it. No host, port,
/// options (user, catalog, scheme, TLS), secret reference, status or engine id.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DatabaseListDto {
    pub id: String,
    pub name: String,
    pub engine: Engine,
    /// Whether queries on this connection run as the signed-in user.
    pub delegated: bool,
}

impl From<Database> for DatabaseListDto {
    fn from(d: Database) -> Self {
        DatabaseListDto {
            id: d.id.to_string(),
            name: d.name,
            engine: d.engine,
            delegated: d.delegated,
        }
    }
}

/// The body of `GET /ws/{ws}/databases`: the full detail for a workspace admin,
/// the redacted summary for everyone else who may list.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(untagged)]
pub enum DatabaseListing {
    Full(Vec<DatabaseDetailDto>),
    Summary(Vec<DatabaseListDto>),
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
/// `storage_name` is the storage connection THIS catalog's own warehouse writes
/// to — `null` for a jdbc or generic catalog, which has no warehouse and no
/// storage. `storage_name`/`secret_name` are best-effort (`null` when
/// unresolvable); the secret VALUE is never exposed.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct WsEngineCatalogDto {
    pub name: String,
    pub kind: String,
    pub storage_name: Option<String>,
    pub secret_name: Option<String>,
}

/// Read-only detail of an assigned engine: its catalogs, each resolved to the
/// storage connection of its own warehouse (none for a jdbc/generic catalog)
/// and the secret name it uses.
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
