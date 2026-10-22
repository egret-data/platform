//! Connector trait + per-engine construction + value mapping.

use async_trait::async_trait;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use crate::registry::model::{ConnOptions, Database, Engine};

use egret_core::{ColumnMeta, TabularResult};

use crate::execution::error::EngineError;
use crate::execution::model::Limits;
use egret_query_api::Param;

mod http_pool;
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
    /// Stable per-user identity to impersonate (Trino `X-Trino-User`), when the
    /// connection's `impersonate` flag is on. `None` = no end-user resolved.
    pub session_user: Option<String>,
    /// Per-user OIDC engine JWT presented to the engine as `Authorization: Bearer`.
    /// When set, it takes precedence over `secret` (basic-auth). `None` = legacy path.
    pub bearer_token: Option<String>,
}

impl ConnSpec {
    /// Build from a resolved registry `Database` + its opened secret.
    /// No end-user identity is attached (`session_user = None`, `bearer_token = None`).
    pub fn from_database(db: &Database, secret: Option<String>) -> Self {
        Self::from_database_as(db, secret, None, None)
    }

    /// Like `from_database`, but attaches a stable per-user `session_user`
    /// for Trino impersonation (used only when the connection opts in), and
    /// optionally a per-user OIDC engine JWT.
    pub fn from_database_as(
        db: &Database,
        secret: Option<String>,
        session_user: Option<String>,
        bearer_token: Option<String>,
    ) -> Self {
        ConnSpec {
            engine: db.engine,
            host: db.host.clone(),
            port: db.port,
            options: db.options.clone(),
            secret,
            session_user,
            bearer_token,
        }
    }
}

#[async_trait]
pub trait Connector: Send + Sync {
    /// Run `sql`, honoring `limits` and `cancel`. Fetches up to `max_rows + 1` rows;
    /// if the extra row appears, sets `truncated` and drops it. On a triggered
    /// `cancel`, best-effort aborts the engine query and returns `EngineError::Cancelled`.
    ///
    /// Cancellation is observed cooperatively between fetched pages/rows; a query
    /// blocked in a single network read is interrupted by the JobManager's outer
    /// timeout, which drops the future.
    async fn execute(
        &self,
        sql: &str,
        limits: &Limits,
        cancel: CancellationToken,
    ) -> Result<TabularResult, EngineError>;

    /// Return the typed columns of an arbitrary `SELECT` without fetching rows.
    /// Default: wrap the query as `LIMIT 0` and reuse `execute`'s column metadata —
    /// correct for engines whose `execute` already yields clean type names (Trino).
    /// Engines whose `execute` emits protocol-level type tags override this.
    async fn describe(&self, sql: &str, timeout: Duration) -> Result<Vec<ColumnMeta>, EngineError> {
        let wrapped = format!("SELECT * FROM (\n{sql}\n) AS _egret_ds LIMIT 0");
        let limits = Limits {
            max_rows: 0,
            timeout,
        };
        // The wrapper puts the user's SQL on line 2: report the engine's
        // positions on the user's lines (KI-087).
        let r = self
            .execute(&wrapped, &limits, CancellationToken::new())
            .await
            .map_err(|e| match e {
                EngineError::Engine(m) => {
                    EngineError::Engine(crate::execution::unshift_error_lines(&m))
                }
                other => other,
            })?;
        Ok(r.columns)
    }

    /// Run `sql` with `params` bound by the engine's own parameter mechanism.
    ///
    /// The default REFUSES rather than interpolating. That asymmetry is the point:
    /// a defaulted `params` argument on `execute` would force every implementor —
    /// including test fakes and any future connector — to make a decision, and the
    /// cheapest wrong decision is to ignore the argument and build a string. This
    /// codebase already lost a SEV-0 to a defaulted inherited behaviour in `describe`.
    async fn execute_with_params(
        &self,
        sql: &str,
        params: &[Param],
        limits: &Limits,
        cancel: CancellationToken,
    ) -> Result<TabularResult, EngineError> {
        if params.is_empty() {
            self.execute(sql, limits, cancel).await
        } else {
            Err(EngineError::Engine(
                "this connector does not support bound parameters".into(),
            ))
        }
    }

    /// Typed columns of a parameterised `SELECT`, without supplying values.
    ///
    /// Not symmetry: on StarRocks the text path given SQL containing `?` returns
    /// ZERO COLUMNS AND NO ERROR (measured), so publish would freeze an empty
    /// schema instead of failing.
    async fn describe_with_params(
        &self,
        sql: &str,
        param_count: usize,
        timeout: Duration,
    ) -> Result<Vec<ColumnMeta>, EngineError> {
        if param_count == 0 {
            self.describe(sql, timeout).await
        } else {
            Err(EngineError::Engine(
                "this connector does not support describing a parameterised query".into(),
            ))
        }
    }
}

/// Construct the right connector for a spec.
pub fn build_connector(spec: &ConnSpec) -> Box<dyn Connector> {
    match spec.engine {
        Engine::Trino => Box::new(trino::TrinoConnector::new(spec)),
        Engine::StarRocks => Box::new(starrocks::StarRocksConnector::new(spec)),
    }
}

/// The one statement shape [`run_lineage_explain`] sends, spelled once in
/// `egret_perm_compile::lineage`, whose `explain_only` prepends it to a view's
/// SQL and refuses a SQL that is itself an `EXPLAIN` (`EXPLAIN ANALYZE`
/// executes its statement).
pub use egret_perm_compile::lineage::EXPLAIN_IO_PREFIX;

/// Run one `EXPLAIN (TYPE IO, FORMAT JSON) …` on a Trino coordinator, with
/// `catalog` (and `schema`, when not empty) as the session defaults — view
/// governance B1, the view-lineage computation.
///
/// Refuses, before any request, a non-Trino `spec` and a `sql` that does not
/// begin with [`EXPLAIN_IO_PREFIX`]. That prefix check is not what keeps the
/// principal read-only: Trino's statement protocol runs exactly ONE statement
/// per request, so the text is an EXPLAIN or a parse error, and the principal
/// holds only `use` and `select`. It keeps this function from being a
/// general-purpose way to run SQL as whatever principal `spec` names.
pub async fn run_lineage_explain(
    spec: &ConnSpec,
    catalog: &str,
    schema: &str,
    sql: &str,
    timeout: Duration,
) -> Result<TabularResult, EngineError> {
    if spec.engine != Engine::Trino {
        return Err(EngineError::Engine(
            "the lineage EXPLAIN runs on Trino only".into(),
        ));
    }
    if !sql.starts_with(EXPLAIN_IO_PREFIX) {
        return Err(EngineError::Engine(format!(
            "the lineage client sends only `{}…`",
            EXPLAIN_IO_PREFIX.trim_end()
        )));
    }
    let connector = trino::TrinoConnector::new(spec).with_session(catalog, schema);
    // One row: the plan. A second row would be truncated, and the caller's shape
    // check refuses a plan it cannot read.
    let limits = Limits {
        max_rows: 1,
        timeout,
    };
    let cancel = CancellationToken::new();
    match tokio::time::timeout(timeout, connector.execute(sql, &limits, cancel.clone())).await {
        Ok(r) => r,
        Err(_) => {
            cancel.cancel();
            Err(EngineError::Timeout)
        }
    }
}

/// Run one catalog-listing statement (`SHOW SCHEMAS`, an
/// `information_schema` read, `SHOW TABLES`, `DESCRIBE`) on a Trino
/// coordinator, for `super::catalog_listing` — DG frontend redesign #1.
///
/// Crate-private: the statements are built there from quoted names, and this
/// is not a general way to run SQL as whatever principal `spec` names. Refuses
/// a non-Trino `spec`. Reads at most `max_rows` rows; one more sets
/// `truncated`, which the caller refuses.
pub(crate) async fn run_catalog_listing(
    spec: &ConnSpec,
    sql: &str,
    max_rows: usize,
    timeout: Duration,
) -> Result<TabularResult, EngineError> {
    if spec.engine != Engine::Trino {
        return Err(EngineError::Engine(
            "the catalog listing runs on Trino only".into(),
        ));
    }
    let connector = trino::TrinoConnector::new(spec);
    let limits = Limits { max_rows, timeout };
    let cancel = CancellationToken::new();
    match tokio::time::timeout(timeout, connector.execute(sql, &limits, cancel.clone())).await {
        Ok(r) => r,
        Err(_) => {
            cancel.cancel();
            Err(EngineError::Timeout)
        }
    }
}

/// The off-by-default TLS insecure opt-out, shared with the notebook kernel
/// (`EGRET_ENGINE_TLS_INSECURE`). `1`/`true`/`yes` (case-insensitive) enable it.
/// When set, engine connections skip the transport-security guard below — an
/// explicit, operator-chosen escape hatch, never a silent fallback.
pub fn tls_insecure_optout() -> bool {
    std::env::var("EGRET_ENGINE_TLS_INSECURE")
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

/// Refusal reason when a per-user engine JWT would be transmitted without TLS.
///
/// A bearer token is a credential: Trino sends it as an `Authorization: Bearer`
/// header, StarRocks as a **cleartext** MySQL password (`mysql_clear_password`).
/// Either leaks the JWT verbatim over a non-TLS transport. We pin the engine's
/// cert to provide that TLS, so a bearer connection with no TLS would send the
/// credential in the clear. Fail closed: return `Some(reason)` unless the
/// operator set the explicit `EGRET_ENGINE_TLS_INSECURE` opt-out.
pub(crate) fn insecure_bearer_reason(
    has_bearer: bool,
    transport_tls: bool,
    insecure_optout: bool,
) -> Option<String> {
    if has_bearer && !transport_tls && !insecure_optout {
        Some(
            "refusing to send a per-user engine JWT over a non-TLS connection: the \
             engine connection has no TLS/pinned cert, so the token would be \
             transmitted in the clear. Configure the engine's TLS, or set \
             EGRET_ENGINE_TLS_INSECURE=1 to override (insecure)."
                .to_string(),
        )
    } else {
        None
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
        session_user: Option<String>,
        bearer_token: Option<String>,
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
        session_user: Option<String>,
        bearer_token: Option<String>,
    ) -> Result<Box<dyn Connector>, EngineError> {
        Ok(build_connector(&ConnSpec::from_database_as(
            db,
            secret,
            session_user,
            bearer_token,
        )))
    }
}

#[cfg(test)]
mod guard_tests {
    use super::insecure_bearer_reason as reason;

    #[test]
    fn bearer_without_tls_is_refused() {
        // The dangerous case: a per-user JWT and no TLS, opt-out off → refuse.
        assert!(reason(true, false, false).is_some());
    }

    #[test]
    fn bearer_with_tls_is_allowed() {
        // jwt-mode engines are provisioned with TLS (pinned cert / https) → allowed.
        assert!(reason(true, true, false).is_none());
    }

    #[test]
    fn no_bearer_is_always_allowed() {
        // The impersonate / service-user paths carry no per-user JWT — unaffected,
        // even over plain transport.
        assert!(reason(false, false, false).is_none());
        assert!(reason(false, true, false).is_none());
    }

    #[test]
    fn explicit_optout_permits_insecure_bearer() {
        // The escape hatch: only an explicit opt-out downgrades the refusal.
        assert!(reason(true, false, true).is_none());
    }
}
