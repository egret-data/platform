//! StarRocks connector — MySQL protocol (mysql_async) with row-cap + cancellation.
//!
//! # These connections are deliberately NOT pooled
//!
//! Package 3b introduced connection reuse for Trino — one `reqwest::Client` per
//! coordinator, every credential attached per request (see
//! `connector/http_pool.rs`) — and deliberately did not do the equivalent here.
//! Recorded as a decision with its evidence, because "there is no pool" reads
//! like an omission and this one is not.
//!
//! A `mysql_async::Pool` is keyed by its `Opts`, and recycles a connection by
//! sending `COM_RESET_CONNECTION` (`PoolOpts::reset_connection`, on by default).
//! That is the property that makes a pool *look* safe here. It is not:
//!
//! 1. **`EXECUTE AS <user> WITH NO REVERT` survives the reset.** Measured live
//!    against StarRocks and pinned by
//!    `execute_as_survives_a_connection_reset_which_is_why_starrocks_is_not_pooled`
//!    in `tests/connector_starrocks.rs`: after the reset, `current_user()` is
//!    still the impersonated user. A recycled connection would therefore run the
//!    next request as the **previous principal** — a cross-principal data leak,
//!    and the most serious defect this programme could introduce.
//! 2. **The identity switch is not only ours to make.** SQL Lab and notebooks
//!    send arbitrary user SQL through this same connector, and StarRocks
//!    executes every statement in a `COM_QUERY` payload regardless of the
//!    client's capability flags (see [`reject_statement_separator`]). So a user
//!    can issue `EXECUTE AS` themselves and poison a pooled connection even on
//!    the path where egret never switches identity.
//! 3. **The delegated path has no safe key anyway.** Under
//!    `authentication_jwt` the MySQL login user and password ARE the per-user
//!    identity and its short-lived token, so an `Opts`-keyed pool would hold one
//!    entry per user per token refresh — a cache of expiring credentials rather
//!    than a pool.
//!
//! What would have to change before pooling is reconsidered: (1) must stop being
//! true, or the pool must key on the post-`EXECUTE AS` identity AND be able to
//! prove no user SQL can move it — which (2) says it cannot. Until then the cost
//! stands: one TCP (and, with a pinned FE cert, one TLS) handshake per statement.

use async_trait::async_trait;
use mysql_async::prelude::Queryable;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use egret_core::{ColumnMeta, TabularResult};

use super::{ConnSpec, Connector};
use crate::execution::error::EngineError;
use crate::execution::model::Limits;
use egret_query_api::Param;

/// Refuse any submission that could be split into more than one statement.
///
/// # Why this is a byte test and not a SQL scan
///
/// StarRocks executes EVERY statement in a `COM_QUERY` payload, and it does so
/// **regardless of the client's capability flags**. Measured on live StarRocks
/// 4.1.3 through this connector: clearing `CLIENT_MULTI_STATEMENTS` from the
/// handshake — and then `CLIENT_MULTI_RESULTS` as well — changed nothing.
/// `SELECT 1 AS a; DROP TABLE db.t` still dropped the table and still returned a
/// clean one-column result. So there is no protocol-level "single statement
/// only" mode to ask for; the negotiation MySQL documents is advisory here.
///
/// Nor can the answer be to inspect the response: by the time a second result
/// set (or OK packet) arrives, the `DROP TABLE` has already run. Detection is
/// not prevention.
///
/// That leaves exactly one property that is a PROOF rather than a deny-list: a
/// payload containing no `;` byte cannot be split on `;`. This function tests
/// bytes. It does not lex, so it cannot disagree with the server's lexer — which
/// is the entire failure mode it exists to remove. The Data API's SEV-0 was a
/// scanner that believed `#` opened a comment while StarRocks accepts and
/// IGNORES `#`, letting `#) AS _egret_ds LIMIT 0 ;` close [`Connector::describe`]'s
/// own wrapper and promote the `;` to a top-level separator. Any future
/// scanner-vs-lexer gap would be just as catastrophic; this check is immune to
/// all of them because it makes no claim about what the bytes MEAN.
///
/// # What it costs
///
/// A `;` anywhere but a trailing position is refused **even when it is genuinely
/// inert** — inside a string literal (`WHERE label = 'a;b'`) or a comment. That
/// is a real false rejection, taken deliberately: deciding that a given `;` is
/// inert requires exactly the lexer agreement this function refuses to assume.
/// The author's workaround is `concat('a', char(59), 'b')`, and the refusal is a
/// loud error at authoring time rather than a silent DDL at validate time.
///
/// Trailing separators are trimmed before the test, because a `;` at the end
/// cannot separate anything. That does NOT make `SELECT 1;` describable: the
/// wrapper puts it inside `SELECT * FROM ( … ) AS _egret_ds`, where StarRocks
/// rejects it as a syntax error. That predates this function and is unchanged by
/// it — the trim only keeps the trailing case out of THIS refusal, so the error
/// an author sees names the real cause. Both halves are pinned in
/// `tests/connector_starrocks.rs`.
///
/// # Where it is applied, and where it is NOT
///
/// Applied to [`StarRocksConnector::describe`], whose contract is "tell me the
/// columns of this SELECT" — a caller that asked for introspection has already
/// decided nothing should execute, so a batched DDL there breaks describe's own
/// promise no matter who called it (Data API validate, dataset preview, typegen).
///
/// NOT applied to [`StarRocksConnector::execute`]. That is SQL Lab's deliberate
/// arbitrary-SQL channel — `execution::job::apply_sql_limit` explicitly carries a
/// multi-statement branch — and a caller holding `query:run` on the connection
/// can already send `DROP TABLE` as statement one, so batching is not an
/// escalation there. **The consequence the Data API must not inherit:** a serving
/// path that hands snapshot SQL to `execute` gets no protection from this
/// function.
///
/// # The premise that used to be written here, and how it was falsified
///
/// This comment used to say the Data API was safe because "egret-dataapi's
/// validator refuses a `;` in the RAW definition, so no `;` can reach a published
/// snapshot in the first place". That was true of the definition and stopped
/// being true of the STATEMENT the moment P4.5 started rendering: `| ident`
/// writes an entry from the author's `allowed` list into the statement as text,
/// and `allowed = ["id", "id; DROP TABLE t"]` published green and rendered
/// `SELECT id FROM t ORDER BY id; DROP TABLE t` with zero bound values — which,
/// having no parameters, arrives here through `execute`.
///
/// What keeps it safe NOW is two rules in `egret-dataapi`, neither of them the
/// raw-definition scan: `render::check_rendered_statement` applies this same byte
/// test to the RENDERED statement on every publish and every request, and
/// `lifecycle::check_ident_values` requires every `allowed` entry that can reach
/// an identifier position to be identifier-shaped. If either is ever relaxed,
/// this function's absence from `execute` becomes an escalation again.
fn reject_statement_separator(sql: &str) -> Result<(), EngineError> {
    if sql.trim_end().trim_end_matches(';').contains(';') {
        return Err(EngineError::Engine(
            "refusing to introspect SQL containing a statement separator `;`: StarRocks \
             executes every statement in one submission regardless of client capability \
             flags, so a `;` outside a trailing position cannot be sent safely"
                .to_string(),
        ));
    }
    Ok(())
}

pub struct StarRocksConnector {
    opts: mysql_async::Opts,
    /// Set on the impersonate path (service-user login + run AS this user).
    execute_as: Option<String>,
    /// Set (to a refusal message) when this connection carries a per-user JWT but
    /// no TLS — StarRocks sends the JWT as a *cleartext* MySQL password, so without
    /// TLS the token leaks verbatim. Enforced in `execute`/`describe` (fail closed).
    insecure_bearer: Option<String>,
}

impl StarRocksConnector {
    pub fn new(spec: &ConnSpec) -> Self {
        // Under delegation (a bearer is present), StarRocks `authentication_jwt`
        // requires the MySQL login user to equal the JWT principal
        // (`preferred_username`) — so use the resolved per-user `session_user`,
        // mirroring Trino's `X-Trino-User = session_user`. Without a bearer the
        // connection's configured service user is used (unchanged behavior).
        let user = if spec.bearer_token.is_some() {
            spec.session_user
                .clone()
                .or_else(|| spec.options.user.clone())
                .unwrap_or_else(|| "root".into())
        } else {
            spec.options.user.clone().unwrap_or_else(|| "root".into())
        };
        let mut b = mysql_async::OptsBuilder::default()
            .ip_or_hostname(spec.host.clone())
            .tcp_port(u16::try_from(spec.port).unwrap_or(9030))
            .user(Some(user))
            .prefer_socket(false);
        if let Some(jwt) = &spec.bearer_token {
            // StarRocks `authentication_jwt`: present the OIDC JWT as the cleartext
            // "password" via the mysql_clear_password client plugin. mysql_async sends
            // it verbatim (unhashed) when the server requests that plugin.
            b = b.pass(Some(jwt.clone())).enable_cleartext_plugin(true);
        } else if let Some(s) = &spec.secret {
            b = b.pass(Some(s.clone()));
        }
        if let Some(db) = &spec.options.database {
            b = b.db_name(Some(db.clone()));
        }
        // Impersonate mode (Trino parity): no bearer, connection opts in, end-user resolved.
        // Log in as the service user (above) and switch identity per-query in `execute`
        // via `EXECUTE AS <session_user> WITH NO REVERT`.
        let execute_as = if spec.bearer_token.is_none() && spec.options.impersonate == Some(true) {
            spec.session_user.clone()
        } else {
            None
        };
        // TLS: pin the FE cert. mysql_clear_password sends the JWT in cleartext, so a
        // bearer connection MUST be TLS — enforced (fail closed) via `insecure_bearer`
        // below, no longer just a comment.
        let has_tls = spec.options.tls_cert.is_some();
        if let Some(pem) = &spec.options.tls_cert {
            let ssl = mysql_async::SslOpts::default()
                .with_root_certs(vec![mysql_async::PathOrBuf::from(pem.clone().into_bytes())])
                // Safe: we pin the exact self-signed FE leaf cert as the trust root, so the
                // chain/signature are still validated (via `with_root_certs`) — only
                // hostname/SAN matching is skipped, needed because the FE's self-signed cert
                // CN won't match the in-cluster Service DNS we dial.
                .with_danger_skip_domain_validation(true);
            b = b.ssl_opts(Some(ssl));
        }
        StarRocksConnector {
            opts: b.into(),
            execute_as,
            // StarRocks' only TLS path here is the pinned FE cert, so the transport is
            // TLS iff `tls_cert` was provided. Without it, the cleartext JWT password
            // would go over a plaintext socket → refuse.
            insecure_bearer: super::insecure_bearer_reason(
                spec.bearer_token.is_some(),
                has_tls,
                super::tls_insecure_optout(),
            ),
        }
    }

    /// The transport-security refusal, if this connection would leak a bearer JWT
    /// (StarRocks sends it as a cleartext password) over a non-TLS socket.
    fn guard(&self) -> Result<(), EngineError> {
        match &self.insecure_bearer {
            Some(reason) => Err(EngineError::Connect(reason.clone())),
            None => Ok(()),
        }
    }

    /// Switch the connection's session identity to `user` (impersonate mode).
    /// Backticks in the identifier are doubled per the MySQL identifier grammar.
    async fn switch_identity(conn: &mut mysql_async::Conn, user: &str) -> Result<(), EngineError> {
        let esc = user.replace('`', "``");
        conn.query_drop(format!("EXECUTE AS `{esc}` WITH NO REVERT"))
            .await
            .map_err(|e| EngineError::Engine(e.to_string()))
    }

    #[cfg(test)]
    pub fn opts(&self) -> &mysql_async::Opts {
        &self.opts
    }

    #[cfg(test)]
    pub fn execute_as(&self) -> Option<&str> {
        self.execute_as.as_deref()
    }

    #[cfg(test)]
    pub fn insecure_bearer(&self) -> Option<&str> {
        self.insecure_bearer.as_deref()
    }
}

#[async_trait]
impl Connector for StarRocksConnector {
    async fn execute(
        &self,
        sql: &str,
        limits: &Limits,
        cancel: CancellationToken,
    ) -> Result<TabularResult, EngineError> {
        self.guard()?;
        let mut conn = mysql_async::Conn::new(self.opts.clone())
            .await
            .map_err(|e| EngineError::Connect(e.to_string()))?;

        if let Some(u) = &self.execute_as {
            Self::switch_identity(&mut conn, u).await?;
        }

        // Text protocol: dates/decimals/strings all arrive as `Value::Bytes`,
        // decoded by `bytes_to_json` using the declared column type.
        let mut result = conn
            .query_iter(sql)
            .await
            .map_err(|e| EngineError::Engine(e.to_string()))?;

        let drained = drain_rows(&mut result, limits.max_rows, &cancel, false).await;
        drop(result);
        let _ = conn.disconnect().await;
        let (columns, rows, truncated) = drained?;

        Ok(TabularResult {
            columns,
            row_count: rows.len() as u64,
            rows,
            bytes_scanned: None,
            truncated,
        })
    }

    /// Bind `params` via `mysql_async`'s prepared-statement (binary protocol)
    /// path: `prep` then `exec_iter`. Every param is carried as text
    /// (`Value::Bytes`) or a native numeric — StarRocks reports every bound
    /// parameter as `MYSQL_TYPE_VAR_STRING` and does no type inference of its
    /// own, so the `CAST(? AS ...)` in the caller's SQL is what gives a bound
    /// value its type, not the wire representation here.
    ///
    /// `values` is built by mapping `params` 1:1 — never by separately
    /// deriving an expected placeholder count from the SQL text and a value
    /// count from `params`. A StarRocks `StmtParamsMismatch` kills the whole
    /// connection (a later `SELECT 1` on it fails with "Connection to the
    /// server is closed"), so those two counts must never be able to disagree.
    ///
    /// **Not timeout-bound here.** Like `execute`, this relies entirely on an
    /// outer caller to enforce `limits.timeout` by dropping the future —
    /// `limits.timeout` itself is unread here. Whoever calls this MUST supply
    /// that outer `tokio::time::timeout` wrapper, or a slow StarRocks query
    /// hangs the request indefinitely. Dropping the future closes the
    /// connection, and StarRocks 4.1.6 cancels the statement on its own
    /// (measured 2026-10-17, pinned by
    /// `starrocks_cancels_a_select_when_egret_drops_it` and
    /// `starrocks_cancels_a_parameterised_select_when_egret_drops_it`).
    async fn execute_with_params(
        &self,
        sql: &str,
        params: &[Param],
        limits: &Limits,
        cancel: CancellationToken,
    ) -> Result<TabularResult, EngineError> {
        if params.is_empty() {
            return self.execute(sql, limits, cancel).await;
        }
        self.guard()?;
        let mut conn = mysql_async::Conn::new(self.opts.clone())
            .await
            .map_err(|e| EngineError::Connect(e.to_string()))?;

        if let Some(u) = &self.execute_as {
            Self::switch_identity(&mut conn, u).await?;
        }

        let stmt = conn
            .prep(sql)
            .await
            .map_err(|e| EngineError::Engine(e.to_string()))?;

        let values: Vec<mysql_async::Value> = params.iter().map(param_to_value).collect();

        // Binary protocol: DATE/DATETIME now arrive as `Value::Date(..)`, not
        // `Bytes` — decoded by the `V::Date` arm in `row_to_json` below.
        let mut result = conn
            .exec_iter(stmt, values)
            .await
            .map_err(|e| EngineError::Engine(e.to_string()))?;

        let drained = drain_rows(&mut result, limits.max_rows, &cancel, true).await;
        drop(result);
        let _ = conn.disconnect().await;
        let (columns, rows, truncated) = drained?;

        Ok(TabularResult {
            columns,
            row_count: rows.len() as u64,
            rows,
            bytes_scanned: None,
            truncated,
        })
    }

    async fn describe(&self, sql: &str, timeout: Duration) -> Result<Vec<ColumnMeta>, EngineError> {
        self.guard()?;
        // Before the wrapper, not after: the attack closes the wrapper, so a check
        // on `wrapped` would be checking a string the attacker already controls the
        // shape of. See `reject_statement_separator` for why this is a byte test.
        reject_statement_separator(sql)?;
        let wrapped = format!("SELECT * FROM (\n{sql}\n) AS _egret_ds LIMIT 0");
        let opts = self.opts.clone();
        let execute_as = self.execute_as.clone();
        // Bound the whole connect+query under `timeout` — unlike `execute`, `describe`
        // is called directly (no JobManager outer timeout), so a hung query would block.
        let fut = async move {
            let mut conn = mysql_async::Conn::new(opts)
                .await
                .map_err(|e| EngineError::Connect(e.to_string()))?;
            // Impersonate (Trino parity): switch to the end-user before introspecting so
            // dataset preview/describe honors per-user Ranger, matching `execute`.
            if let Some(u) = &execute_as {
                StarRocksConnector::switch_identity(&mut conn, u).await?;
            }
            let result = conn
                .query_iter(wrapped)
                .await
                .map_err(|e| EngineError::Engine(e.to_string()))?;
            let columns: Vec<ColumnMeta> = result
                .columns_ref()
                .iter()
                .map(|c| ColumnMeta {
                    name: c.name_str().to_string(),
                    ty: coltype_name(c.column_type()).to_string(),
                })
                .collect();
            drop(result);
            let _ = conn.disconnect().await;
            Ok::<Vec<ColumnMeta>, EngineError>(columns)
        };
        tokio::time::timeout(timeout, fut)
            .await
            .map_err(|_| EngineError::Timeout)?
    }

    /// Typed columns of a parameterised `SELECT`, without supplying values.
    ///
    /// Not a fallback to [`Connector::describe`]'s text-protocol path — that
    /// is the exact defect this method exists to prevent. Measured live: the
    /// text protocol given a wrapped query containing `?` returns a normal,
    /// *successful* result with **zero columns** (see
    /// `plain_describe_on_parameterised_sql_silently_returns_zero_columns`
    /// in the test file), because StarRocks' `COM_QUERY` path treats `?` as
    /// meaningless syntax it happens not to reject rather than a parameter
    /// marker. Publish-time typing calling that path would freeze an empty
    /// schema into the snapshot with no error to notice.
    ///
    /// Instead this uses `conn.prep()` (the binary/prepared-statement
    /// protocol, `COM_STMT_PREPARE`) on the wrapped query and reads
    /// `Statement::columns()` — measured to return the full, correct output
    /// schema for a query containing `?`. `execute_with_params` already
    /// drives this same protocol for execution; this is its describe-only
    /// counterpart, never executing the statement.
    async fn describe_with_params(
        &self,
        sql: &str,
        param_count: usize,
        timeout: Duration,
    ) -> Result<Vec<ColumnMeta>, EngineError> {
        if param_count == 0 {
            return self.describe(sql, timeout).await;
        }
        self.guard()?;
        // Same reasoning as `describe`: check before wrapping, not after —
        // the wrapper is exactly what the SEV-0 payload closes early.
        reject_statement_separator(sql)?;
        let wrapped = format!("SELECT * FROM (\n{sql}\n) AS _egret_ds LIMIT 0");
        let opts = self.opts.clone();
        let execute_as = self.execute_as.clone();
        let fut = async move {
            let mut conn = mysql_async::Conn::new(opts)
                .await
                .map_err(|e| EngineError::Connect(e.to_string()))?;
            if let Some(u) = &execute_as {
                StarRocksConnector::switch_identity(&mut conn, u).await?;
            }
            let stmt = conn
                .prep(wrapped)
                .await
                .map_err(|e| EngineError::Engine(e.to_string()))?;
            let columns: Vec<ColumnMeta> = stmt
                .columns()
                .iter()
                .map(|c| ColumnMeta {
                    name: c.name_str().to_string(),
                    ty: coltype_name(c.column_type()).to_string(),
                })
                .collect();
            let _ = conn.close(stmt).await;
            let _ = conn.disconnect().await;
            Ok::<Vec<ColumnMeta>, EngineError>(columns)
        };
        tokio::time::timeout(timeout, fut)
            .await
            .map_err(|_| EngineError::Timeout)?
    }
}

/// Drain a pending result set (text or binary protocol — both are `Protocol`
/// impls over the same `QueryResult` API) into columns + JSON rows, applying
/// the row cap and cooperative cancellation. Shared by `execute` and
/// `execute_with_params`, which differ only in how the result was produced.
///
/// `refuse_time` is set only by `execute_with_params` (the binary/`exec_iter`
/// path) — see the `MYSQL_TYPE_TIME`/`MYSQL_TYPE_TIME2` check below for why.
async fn drain_rows<P: mysql_async::prelude::Protocol>(
    result: &mut mysql_async::QueryResult<'_, 'static, P>,
    cap: usize,
    cancel: &CancellationToken,
    refuse_time: bool,
) -> Result<(Vec<ColumnMeta>, Vec<Vec<serde_json::Value>>, bool), EngineError> {
    // Read columns BEFORE consuming any rows — columns_ref() accesses the pending result set.
    let columns: Vec<ColumnMeta> = result
        .columns_ref()
        .iter()
        .map(|c| ColumnMeta {
            name: c.name_str().to_string(),
            // Canonical lowercase SQL name, the same vocabulary the `describe`
            // paths report — NOT `format!("{:?}", ..)`, which leaks raw protocol
            // tags like `MYSQL_TYPE_VAR_STRING` into every query result.
            ty: coltype_name(c.column_type()).to_string(),
        })
        .collect();

    // Capture per-column MySQL types for type-aware value coercion.
    let col_types: Vec<mysql_async::consts::ColumnType> = result
        .columns_ref()
        .iter()
        .map(|c| c.column_type())
        .collect();

    // Measured live: on the binary protocol, StarRocks' TIME payload does not
    // decode as a standard MySQL TIME value — `CAST('10:20:30' AS TIME)` came
    // back with day/hour fields so large that `mysql_common`'s own `Debug`
    // impl for `Value::Time` (which multiplies `days * 24`) panics with
    // "attempt to multiply with overflow". A stock MySQL client reads the same
    // payload as `days=86186786, 21:08:12` — not merely imprecise, wrong. The
    // text protocol (`execute`, no bound params) is unaffected: it returns
    // TIME as `Value::Bytes`, decoded correctly by `bytes_to_json`. Refusing
    // here — before a single row is fetched — is the "clear error" outcome:
    // there is no faithful decode to fall back to, so a plausible-looking
    // wrong string would be strictly worse than refusing.
    if refuse_time {
        use mysql_async::consts::ColumnType::{MYSQL_TYPE_TIME, MYSQL_TYPE_TIME2};
        if let Some(bad) = col_types
            .iter()
            .position(|t| matches!(t, MYSQL_TYPE_TIME | MYSQL_TYPE_TIME2))
        {
            let name = columns.get(bad).map(|c| c.name.as_str()).unwrap_or("?");
            return Err(EngineError::Engine(format!(
                "bound-parameter query returns a TIME column (`{name}`), which StarRocks' \
                 binary protocol does not encode as a decodable MySQL TIME value (measured \
                 — the payload does not survive round-tripping through the standard \
                 driver). Cast it to text in the SQL instead, e.g. `CAST(<col> AS VARCHAR)`."
            )));
        }
    }

    let mut rows: Vec<Vec<serde_json::Value>> = Vec::new();

    loop {
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        match result
            .next()
            .await
            .map_err(|e| EngineError::Engine(e.to_string()))?
        {
            None => break,
            Some(row) => {
                rows.push(row_to_json(row, &col_types));
                if rows.len() > cap {
                    break;
                }
            }
        }
    }

    let truncated = rows.len() > cap;
    if truncated {
        rows.truncate(cap);
    }
    Ok((columns, rows, truncated))
}

/// Map a bound [`Param`] to the `mysql_async::Value` sent over the binary
/// protocol. `F64`/`Decimal`/`Str`/`Date`/`Timestamp` all go over as `Bytes`
/// of their text form: StarRocks reports every bound parameter as
/// `MYSQL_TYPE_VAR_STRING` regardless of the value's Rust variant, so there is
/// no native wire type to prefer for a date or a decimal, and text is what
/// preserves exact precision.
///
/// `F64` in particular goes over as text rather than `Value::Double` (an
/// 8-byte binary float) — **not because a StarRocks/driver bug was
/// confirmed**. A reviewer originally reported that binding `1234567.891` as
/// `Value::Double` returns `1234567.875`, with `? = 1234567.891` evaluating
/// false even under `CAST(? AS DOUBLE)`. Three direct measurements against a
/// live StarRocks 4.1.3 through this exact `exec_iter` path — see
/// `bound_f64_round_trips_exactly_through_a_backend_table_filter` in
/// `tests/connector_starrocks_params.rs`, including a genuine backend
/// table-scan `WHERE v = ?` filter — found no loss; `Value::Double`
/// round-tripped exactly every time. **This finding is retracted**:
/// `1234567.891f64 as f32 as f64 == 1234567.875` exactly, so the reported
/// value is textbook float32 rounding, not a byte-level bug in this driver or
/// engine. The reviewer's own measurement instrument went through an f32
/// somewhere; this connector never has. Routed through text anyway because it
/// is zero-cost and consistent with `Decimal` — JSON cannot express
/// NaN/Infinity, so no `Param::F64` value can fail to round-trip through
/// `{f}` (shortest round-trip `Display`) — but this change fixes nothing
/// demonstrated, and should not be cited as a bugfix.
fn param_to_value(p: &Param) -> mysql_async::Value {
    use mysql_async::Value as V;
    match p {
        Param::Null => V::NULL,
        Param::Bool(b) => V::Int(i64::from(*b)),
        Param::I64(n) => V::Int(*n),
        Param::F64(f) => V::Bytes(format!("{f}").into_bytes()),
        Param::Decimal(s) | Param::Str(s) | Param::Date(s) | Param::Timestamp(s) => {
            V::Bytes(s.as_bytes().to_vec())
        }
    }
}

fn row_to_json(
    row: mysql_async::Row,
    col_types: &[mysql_async::consts::ColumnType],
) -> Vec<serde_json::Value> {
    use mysql_async::Value as V;
    (0..row.len())
        .map(|i| match row.as_ref(i) {
            Some(V::NULL) | None => serde_json::Value::Null,
            Some(V::Int(n)) => serde_json::json!(n),
            Some(V::UInt(n)) => serde_json::json!(n),
            Some(V::Float(f)) => serde_json::json!(f),
            Some(V::Double(f)) => serde_json::json!(f),
            // Text protocol returns everything as Bytes; use the declared column type
            // to decide coercion — prevents silent string→number corruption (e.g. "007"→7).
            Some(V::Bytes(b)) => bytes_to_json(b, col_types.get(i).copied()),
            // Binary protocol only: `query()` returns dates as Bytes, `exec()` returns
            // V::Date. Without this arm they fall through to the Debug-repr catch-all
            // below and reach the frontend as `Date("'2020-02-29'")`. Every test on
            // `bytes_to_json` stays green when that happens, because on this path
            // `bytes_to_json` is no longer the function being called.
            Some(&V::Date(y, mo, d, h, mi, s, us)) => {
                serde_json::Value::String(if h == 0 && mi == 0 && s == 0 && us == 0 {
                    format!("{y:04}-{mo:02}-{d:02}")
                } else if us == 0 {
                    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
                } else {
                    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}.{us:06}")
                })
            }
            Some(other) => serde_json::Value::String(format!("{:?}", other)),
        })
        .collect()
}

/// Coerce a MySQL text-protocol byte string to a JSON scalar using the declared column type.
///
/// - Integer types  → parse as i64, fall back to u64, else string.
/// - Float types    → parse as f64, else string.
/// - Everything else (VARCHAR, DECIMAL, dates, …) → keep as string to preserve exact value.
/// - Non-UTF-8 binary → hex-encode.
fn bytes_to_json(b: &[u8], ty: Option<mysql_async::consts::ColumnType>) -> serde_json::Value {
    use mysql_async::consts::ColumnType::*;
    let Ok(s) = std::str::from_utf8(b) else {
        return serde_json::Value::String(hex::encode(b));
    };
    match ty {
        Some(
            MYSQL_TYPE_TINY | MYSQL_TYPE_SHORT | MYSQL_TYPE_LONG | MYSQL_TYPE_LONGLONG
            | MYSQL_TYPE_INT24 | MYSQL_TYPE_YEAR,
        ) => s
            .parse::<i64>()
            .map(|i| serde_json::json!(i))
            .or_else(|_| s.parse::<u64>().map(|u| serde_json::json!(u)))
            .unwrap_or_else(|_| serde_json::Value::String(s.to_owned())),
        Some(MYSQL_TYPE_FLOAT | MYSQL_TYPE_DOUBLE) => s
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(serde_json::Value::Number)
            .unwrap_or_else(|| serde_json::Value::String(s.to_owned())),
        // DECIMAL/NEWDECIMAL, VARCHAR, STRING, dates, blobs, and everything else:
        // keep as string to preserve exact representation.
        _ => serde_json::Value::String(s.to_owned()),
    }
}

/// Map a MySQL/StarRocks protocol column type to a canonical lowercase SQL name,
/// matching the vocabulary `DESCRIBE` returns so `typegen` classifies both the
/// physical (DESCRIBE) and virtual (describe) paths identically.
fn coltype_name(ty: mysql_async::consts::ColumnType) -> &'static str {
    use mysql_async::consts::ColumnType::*;
    match ty {
        MYSQL_TYPE_TINY => "tinyint",
        MYSQL_TYPE_SHORT => "smallint",
        MYSQL_TYPE_LONG | MYSQL_TYPE_INT24 | MYSQL_TYPE_YEAR => "int",
        MYSQL_TYPE_LONGLONG => "bigint",
        MYSQL_TYPE_FLOAT => "float",
        MYSQL_TYPE_DOUBLE => "double",
        MYSQL_TYPE_DECIMAL | MYSQL_TYPE_NEWDECIMAL => "decimal",
        MYSQL_TYPE_DATE | MYSQL_TYPE_NEWDATE => "date",
        MYSQL_TYPE_DATETIME
        | MYSQL_TYPE_DATETIME2
        | MYSQL_TYPE_TIMESTAMP
        | MYSQL_TYPE_TIMESTAMP2 => "datetime",
        MYSQL_TYPE_TIME | MYSQL_TYPE_TIME2 => "time",
        MYSQL_TYPE_JSON => "json",
        MYSQL_TYPE_VARCHAR | MYSQL_TYPE_VAR_STRING | MYSQL_TYPE_STRING => "varchar",
        _ => "string",
    }
}

#[cfg(test)]
mod tests {
    use super::{coltype_name, StarRocksConnector};
    use crate::execution::connector::ConnSpec;
    use crate::registry::model::{ConnOptions, Engine};

    /// `coltype_name` is the single mapping both the describe paths and the
    /// execute path (`drain_rows`) must use to name a column's type. It is a
    /// pure function of the protocol enum, so this pins the vocabulary without
    /// needing an engine — the guard that catches a call site regressing to
    /// `format!("{:?}", ..)`, which yields raw `MYSQL_TYPE_*` protocol tags.
    ///
    /// The names matter beyond display: `egret_bi::typegen::classify_starrocks`
    /// matches on exactly these lowercase keywords and has no `mysql_type_*`
    /// arm, so a debug-formatted type would silently classify as `String`.
    #[test]
    fn coltype_name_maps_protocol_types_to_canonical_sql_names() {
        use mysql_async::consts::ColumnType::*;
        for (ty, want) in [
            (MYSQL_TYPE_TINY, "tinyint"),
            (MYSQL_TYPE_SHORT, "smallint"),
            (MYSQL_TYPE_LONG, "int"),
            (MYSQL_TYPE_INT24, "int"),
            (MYSQL_TYPE_LONGLONG, "bigint"),
            (MYSQL_TYPE_FLOAT, "float"),
            (MYSQL_TYPE_DOUBLE, "double"),
            (MYSQL_TYPE_NEWDECIMAL, "decimal"),
            (MYSQL_TYPE_DATE, "date"),
            (MYSQL_TYPE_DATETIME, "datetime"),
            (MYSQL_TYPE_TIMESTAMP, "datetime"),
            (MYSQL_TYPE_TIME, "time"),
            (MYSQL_TYPE_JSON, "json"),
            (MYSQL_TYPE_VAR_STRING, "varchar"),
            (MYSQL_TYPE_STRING, "varchar"),
        ] {
            assert_eq!(
                coltype_name(ty),
                want,
                "protocol type {ty:?} must map to the canonical SQL name {want:?}"
            );
        }
    }

    /// Nothing `coltype_name` returns may look like a protocol tag, whatever
    /// the input — including the `_ => "string"` fallback for types StarRocks
    /// never sends. This is the property the display layer depends on.
    #[test]
    fn coltype_name_never_returns_a_protocol_tag() {
        use mysql_async::consts::ColumnType::*;
        for ty in [
            MYSQL_TYPE_TINY,
            MYSQL_TYPE_LONG,
            MYSQL_TYPE_NEWDECIMAL,
            MYSQL_TYPE_DATE,
            MYSQL_TYPE_VAR_STRING,
            // Not produced by StarRocks — exercises the fallback arm.
            MYSQL_TYPE_GEOMETRY,
            MYSQL_TYPE_BLOB,
            MYSQL_TYPE_BIT,
        ] {
            let name = coltype_name(ty);
            assert!(
                !name.starts_with("MYSQL_TYPE") && !name.starts_with("mysql_type"),
                "{ty:?} produced a raw protocol tag {name:?}"
            );
            assert_eq!(
                name,
                name.to_ascii_lowercase(),
                "{ty:?} produced {name:?}, which is not lowercase — \
                 typegen's classifiers key off lowercase keywords"
            );
        }
    }

    fn spec(bearer: Option<String>, secret: Option<String>) -> ConnSpec {
        ConnSpec {
            engine: Engine::StarRocks,
            host: "h".into(),
            port: 9030,
            options: ConnOptions {
                user: Some("alice".into()),
                ..Default::default()
            },
            secret,
            session_user: Some("alice".into()),
            bearer_token: bearer,
        }
    }

    #[test]
    fn bearer_uses_cleartext_plugin_with_jwt_as_password() {
        let c =
            StarRocksConnector::new(&spec(Some("eyJ.jwt.sig".into()), Some("native-pw".into())));
        assert!(c.opts().enable_cleartext_plugin());
        assert_eq!(c.opts().pass(), Some("eyJ.jwt.sig"));
    }

    #[test]
    fn no_bearer_keeps_native_password_no_cleartext() {
        let c = StarRocksConnector::new(&spec(None, Some("native-pw".into())));
        assert!(!c.opts().enable_cleartext_plugin());
        assert_eq!(c.opts().pass(), Some("native-pw"));
    }

    /// Build a spec whose configured service user differs from the per-user
    /// `session_user`, so the two login-user branches are distinguishable.
    fn spec_distinct(bearer: Option<String>) -> ConnSpec {
        ConnSpec {
            engine: Engine::StarRocks,
            host: "h".into(),
            port: 9030,
            options: ConnOptions {
                user: Some("svc".into()),
                ..Default::default()
            },
            secret: Some("native-pw".into()),
            session_user: Some("alice".into()),
            bearer_token: bearer,
        }
    }

    #[test]
    fn bearer_login_user_is_session_user() {
        // Delegation: the MySQL login must be the JWT principal, not the service user.
        let c = StarRocksConnector::new(&spec_distinct(Some("eyJ.jwt.sig".into())));
        assert_eq!(c.opts().user(), Some("alice"));
    }

    #[test]
    fn no_bearer_login_user_is_service_user() {
        let c = StarRocksConnector::new(&spec_distinct(None));
        assert_eq!(c.opts().user(), Some("svc"));
    }

    fn base_spec() -> ConnSpec {
        ConnSpec {
            engine: Engine::StarRocks,
            host: "fe".into(),
            port: 9030,
            options: ConnOptions::default(),
            secret: Some("svc-pw".into()),
            session_user: None,
            bearer_token: None,
        }
    }

    #[test]
    fn impersonate_sets_execute_as_and_service_login() {
        let mut s = base_spec();
        s.options.impersonate = Some(true);
        s.options.user = Some("svc".into());
        s.session_user = Some("alice".into());
        let c = StarRocksConnector::new(&s);
        assert_eq!(c.execute_as(), Some("alice"));
        assert_eq!(c.opts().user(), Some("svc"));
    }

    #[test]
    fn jwt_mode_logs_in_as_user_no_execute_as() {
        let mut s = base_spec();
        s.bearer_token = Some("jwt".into());
        s.session_user = Some("alice".into());
        let c = StarRocksConnector::new(&s);
        assert_eq!(c.execute_as(), None);
        assert_eq!(c.opts().user(), Some("alice"));
    }

    #[test]
    fn tls_cert_enables_ssl() {
        let mut s = base_spec();
        s.options.tls_cert =
            Some("-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n".into());
        assert!(StarRocksConnector::new(&s).opts().ssl_opts().is_some());
    }

    // StarRocks sends the JWT as a CLEARTEXT password, so a bearer connection
    // without TLS (no pinned cert) would leak it → flagged insecure (execute/
    // describe fail closed). With the pinned cert it's fine; no bearer is never
    // flagged. (Assumes EGRET_ENGINE_TLS_INSECURE unset, the default.)
    #[test]
    fn bearer_requires_tls() {
        // bearer, no tls_cert → refused
        let mut s = base_spec();
        s.bearer_token = Some("eyJ.jwt.sig".into());
        s.session_user = Some("alice".into());
        assert!(StarRocksConnector::new(&s).insecure_bearer().is_some());

        // bearer + pinned cert → allowed
        s.options.tls_cert =
            Some("-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n".into());
        assert!(StarRocksConnector::new(&s).insecure_bearer().is_none());

        // no bearer (service user / impersonate), no tls → never flagged
        let plain = base_spec();
        assert!(StarRocksConnector::new(&plain).insecure_bearer().is_none());
    }

    /// The byte test, pinned in both directions so it cannot quietly become a
    /// no-op (accept everything) or a blanket ban (reject the ordinary trailing
    /// `;` every SQL editor emits).
    #[test]
    fn statement_separator_is_refused_but_a_trailing_one_is_not() {
        use super::reject_statement_separator as chk;
        // The ordinary shapes must still pass, or describe breaks for everyone.
        assert!(chk("SELECT 1 AS a").is_ok());
        assert!(chk("SELECT 1 AS a;").is_ok());
        assert!(chk("SELECT 1 AS a;  \n").is_ok());
        assert!(chk("SELECT 1 AS a;;").is_ok());

        // A real separator.
        assert!(chk("SELECT 1 AS a; DROP TABLE t").is_err());

        // The SEV-0 payload itself: `#` is accepted and IGNORED by StarRocks, so
        // `#)` closes `describe`'s own wrapper and the `;` becomes top-level.
        // Measured to drop the table before this check existed.
        assert!(chk(
            "SELECT 1 AS a #) AS _egret_ds LIMIT 0 ; DROP TABLE revtest.t ; SELECT * FROM (SELECT 1"
        )
        .is_err());

        // The documented false rejections — an inert `;` in a literal or a
        // comment is refused too, because knowing it is inert needs the lexer
        // agreement this check exists in order not to assume.
        assert!(chk("SELECT * FROM t WHERE label = 'a;b'").is_err());
        assert!(chk("SELECT 1 -- a; b\nFROM t").is_err());
    }

    #[test]
    fn no_tls_cert_no_ssl() {
        assert!(StarRocksConnector::new(&base_spec())
            .opts()
            .ssl_opts()
            .is_none());
    }
}
