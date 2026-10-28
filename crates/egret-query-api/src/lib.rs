//! `egret-query-api` — leaf contract crate for the Query seam (E4d).
//!
//! Holds the shared domain vocabulary — [`Engine`], [`ConnOptions`],
//! [`Database`] — that both `egret-query` (the seam) and its cross-seam
//! consumers (e.g. `egret-bi`) need, without forcing consumers onto deep
//! `egret_query::registry::model::…` paths or pulling in the seam's sqlx/axum
//! machinery. Also re-exports the core tabular result type so the seam's result
//! shape is reachable via the contract crate too.
//!
//! No sqlx `FromRow`/`Type` and no axum. Pure-serde DTOs plus two behavioural
//! exceptions, both deliberate: the dialect-escaping primitive both query
//! consumers must share ([`dialect::quote_string`]), and [`QueryExecPort`] —
//! the seam the Data API reaches execution through, so it never depends on
//! `egret-query` itself.
//!
//! `quote_string` lives here on purpose: it is the single implementation of the
//! SQL-literal escaping rule that every consumer (`egret-query`'s SQL Lab
//! templating, `egret-bi`'s chart/dashboard filters, `egret-engine-mgmt`'s
//! StarRocks `CREATE USER` DDL) must share, and this leaf contract crate is the
//! only place all of them can reach. Do not move or delete it as misplaced —
//! two copies of that rule are what let the escaping drift apart before.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The unified tabular query result + column metadata, re-exported from
/// `egret-core` so consumers can reach the Query seam's result shape through the
/// contract crate.
pub use egret_core::{ColumnMeta, TabularResult};

pub mod dialect;
pub use dialect::{quote_ident, quote_string};

pub mod catalog_listing;

pub mod port;
pub use port::QueryExecPort;

pub mod owner;
pub use owner::DatabaseOwnerPort;

pub mod failure;
pub use failure::{ExecFailure, ExecFailureKind};

/// Execution guards for one query: the row cap and the wall-clock budget.
///
/// Lives in the contract crate because [`QueryExecPort::execute_as`] takes it,
/// and the whole point of that port is that the Data API can name it without
/// depending on `egret-query`. `egret_query::execution::model` re-exports this
/// exact type, so the seam's callers (the JobManager, the BI chart runner, the
/// connection tester) are unchanged — one definition, two paths. Do not add a
/// second `Limits` in the seam: two that drift is the failure mode this crate
/// exists to prevent, the same reason `quote_string` lives here.
///
/// `max_rows` is a cap the connector detects rather than obeys silently: it
/// fetches `max_rows + 1` and, if the extra row appears, sets
/// [`TabularResult::truncated`] and drops it — so a caller can always tell a
/// complete answer from a prefix of one.
#[derive(Debug, Clone)]
pub struct Limits {
    pub max_rows: usize,
    pub timeout: std::time::Duration,
}

/// A caller-supplied value bound into a query. Never rendered into SQL text by
/// the caller of a connector — the connector decides how its engine carries it.
///
/// Lives here, beside [`Limits`], because `egret-dataapi` must be able to
/// construct one without depending on `egret-query`.
#[derive(Debug, Clone, PartialEq)]
pub enum Param {
    Null,
    Bool(bool),
    I64(i64),
    F64(f64),
    /// Exact decimal, kept as text so no float rounding is introduced.
    Decimal(String),
    Str(String),
    /// `YYYY-MM-DD`
    Date(String),
    /// `YYYY-MM-DD HH:MM:SS[.fff]`
    Timestamp(String),
}

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
    /// Run each query as the logged-in user (impersonate delegation mode).
    /// Trino → `X-Trino-User`; StarRocks → `EXECUTE AS <user> WITH NO REVERT`.
    /// Requires engine impersonation + Ranger. Default false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impersonate: Option<bool>,
    /// PEM of the engine's self-signed cert to pin as a trusted root (server-auth
    /// over TLS). Set on egret-provisioned TLS connections — Trino jwt/TLS mode
    /// (https) and StarRocks delegation (in-band TLS on :9030).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_cert: Option<String>,
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
    /// The data behind this connection is governed at the ENGINE — Ranger row
    /// filters and column masks apply to it.
    ///
    /// Consumed by `egret-dataapi`, which refuses to publish an endpoint onto a
    /// governed connection unless [`Database::delegated`] or
    /// `options.impersonate` makes the engine run each query as the CALLER. On a
    /// shared service credential those filters are evaluated for the machine
    /// identity, so every consumer of the endpoint reads what the service
    /// account may read.
    ///
    /// `#[serde(default)]` for rows and payloads written before the column
    /// existed: `false` is what migration 0130 gave every existing row, and it
    /// is the value that changes nothing. It is NOT the fail-safe direction and
    /// is not claimed to be — the gate is opt-in per database by design (see the
    /// migration's own comment for why the alternative was a migration-time
    /// outage).
    #[serde(default)]
    pub governed: bool,
    /// Set when this row was materialized by an engine→workspace assignment.
    /// A non-null `engine_id` marks the connection provisioned (read-only,
    /// per-user-delegated) and links it back to the source engine.
    #[serde(default)]
    pub engine_id: Option<Uuid>,
}

/// Wire-name guards for fields another crate reads out of jsonb **by string**.
///
/// # Why this lives here and not with the reader
///
/// `egret-dataapi` deliberately depends on no other feature seam, so it cannot
/// name [`ConnOptions`]. Its `lifecycle::impersonate_flag` therefore reaches into
/// the raw `query.databases.options` JSONB and looks for a key spelled
/// `"impersonate"`. It has a test of its own — but that test can only assert that
/// `egret-dataapi` still spells what `egret-dataapi` always spelled. **It cannot
/// see a rename here**, which is precisely the change that would break it: add a
/// `#[serde(rename = …)]` or rename the field below and every consumer keeps
/// compiling while the flag silently starts reading `false`.
///
/// The guard belongs where the rename happens, so it goes red at the point of the
/// rename, raised by the crate that owns the type.
///
/// # What reading `false` costs, so the next person knows what they are unpinning
///
/// `impersonate` is one half of the pair that decides whether the engine runs a
/// query **as the caller** — Trino sends `X-Trino-User: <caller>` and StarRocks
/// issues `EXECUTE AS <caller> WITH NO REVERT` on it, independently of
/// `delegated`. The Data API refuses a `shared` response cache on any such
/// connection, because a shared cache stores one principal's Ranger-filtered rows
/// under a principal-less key and serves them to another. If this key stops being
/// found, that refusal silently stops happening and the cross-identity leak comes
/// back with a 200 and `meta.cached: true`.
///
/// So: if you rename the field, update `egret-dataapi`'s `impersonate_flag` in
/// the same change. The real fix is a shared type across the two crates, which is
/// deliberately not done — it would cross the seam boundary that keeps
/// `egret-dataapi` free of `egret-query`, and that is a P5-or-later architectural
/// call rather than something to slip in behind a test.
#[cfg(test)]
mod wire_names {
    use super::*;

    #[test]
    fn conn_options_serialises_impersonate_under_that_exact_key() {
        let opts = ConnOptions {
            impersonate: Some(true),
            ..Default::default()
        };
        let v = serde_json::to_value(&opts).expect("serialise ConnOptions");
        assert_eq!(
            v.get("impersonate"),
            Some(&serde_json::Value::Bool(true)),
            "`ConnOptions::impersonate` must serialise under the literal key \
             `impersonate`: egret-dataapi's `lifecycle::impersonate_flag` reads it \
             out of jsonb by that name and cannot see a rename here. If this is \
             failing because you renamed the field, fix that function in the same \
             change — otherwise a `shared` response cache is silently permitted on \
             a connection that runs queries as the caller. Serialised as: {v}"
        );
    }

    /// The false/absent case, so the guard covers what the reader actually does:
    /// `impersonate_flag` treats anything that is not JSON `true` as `false`, and
    /// `skip_serializing_if` means an unset option is ABSENT rather than `null`.
    /// Both readings must stay "not impersonating".
    #[test]
    fn an_unset_impersonate_is_absent_rather_than_a_surprising_value() {
        let v = serde_json::to_value(ConnOptions::default()).expect("serialise");
        assert_eq!(
            v.get("impersonate"),
            None,
            "an unset `impersonate` must be absent, not null or false-as-string: {v}"
        );
        let v = serde_json::to_value(&ConnOptions {
            impersonate: Some(false),
            ..Default::default()
        })
        .expect("serialise");
        assert_eq!(
            v.get("impersonate"),
            Some(&serde_json::Value::Bool(false)),
            "an explicit false must be a JSON bool, not a string: {v}"
        );
    }

    /// `delegated` is the other half of the pair, and it lives on [`Database`]
    /// rather than in the options blob. Pinned in the same place so the two halves
    /// are one story: whoever renames either finds the same explanation.
    #[test]
    fn database_serialises_delegated_under_that_exact_key() {
        let db = Database {
            id: Uuid::nil(),
            name: String::new(),
            engine: Engine::Trino,
            host: String::new(),
            port: 0,
            options: ConnOptions::default(),
            has_secret: false,
            status: String::new(),
            delegated: true,
            governed: true,
            engine_id: None,
        };
        let v = serde_json::to_value(&db).expect("serialise Database");
        assert_eq!(
            v.get("delegated"),
            Some(&serde_json::Value::Bool(true)),
            "`delegated` is the other half of the pair the Data API's shared-cache \
             witness freezes; it is read by name in SQL (`SELECT delegated, \
             options ...`), so a rename is a schema change and not only a serde \
             one: {v}"
        );
        // The third column of the same story, added by Package 1. It is read by
        // name in SQL from THREE places in `egret-dataapi` — the publish
        // transaction, `EndpointStore::connection_safety`, and
        // `serving::governance::LiveConnection::load` — and none of them would
        // fail to compile on a rename here.
        assert_eq!(
            v.get("governed"),
            Some(&serde_json::Value::Bool(true)),
            "`governed` decides whether the Data API will publish onto this \
             connection at all, and is read by name in SQL (`SELECT delegated, \
             governed, options ...`): a rename is a schema change, and the \
             direction it fails in is that every governed database silently \
             reads as ungoverned: {v}"
        );
    }
}
