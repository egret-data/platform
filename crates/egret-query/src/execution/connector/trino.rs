//! Trino connector — REST statement protocol with row-cap, timeout, cancellation.
//!
//! # Bound parameters are NOT wire-level binding (see spec §3.1)
//!
//! Trino's HTTP statement protocol has no parameter channel. A prepared
//! statement's SQL text travels in the `X-Trino-Prepared-Statement` request
//! header, and the values travel in the POST body as `EXECUTE <name> USING
//! <literal>, <literal>, …` — **SQL text egret renders**, not bytes the wire
//! protocol carries separately. The parser boundary at that endpoint is real
//! and measured: a second statement (`'a'; DROP TABLE zz`) and a subquery
//! (`'a' || (SELECT …)`) are both refused with a parse/analysis error. But a
//! well-formed *scalar expression* is not — `'a' || 'b'` evaluates to `"ab"`,
//! `current_user` evaluates to the caller, `upper('ab')` evaluates to `"AB"`.
//! So a defect in [`render_literal`] is **arbitrary-scalar-expression
//! injection**, not arbitrary SQL and not nothing. `quote_string` (shared
//! with `egret-bi`'s interpolating surfaces) is what closes the quote/quote
//! case; it does not need to double backslashes here because Trino does not
//! treat `\` as a string escape character (unlike StarRocks/MySQL dialect).

use async_trait::async_trait;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use egret_core::{ColumnMeta, TabularResult};

use super::{ConnSpec, Connector};
use crate::execution::error::EngineError;
use crate::execution::model::Limits;
use egret_query_api::{quote_string, Engine, Param};

/// Render a single [`Param`] as Trino SQL text suitable for one position in an
/// `EXECUTE <name> USING <literal>, <literal>, …` list.
///
/// `Date`/`Timestamp`/`Decimal` are rendered as **typed literals**
/// (`DATE '…'`, `TIMESTAMP '…'`, `DECIMAL '…'`) rather than bare quoted text,
/// so the value keeps its declared type through Trino's own parser — a bare
/// `'2020-02-29'` would arrive as `varchar`, not `date`. `F64` is rendered as
/// a typed `DOUBLE '…'` literal for the same reason: an untyped literal like
/// `1.5` is Trino's `DECIMAL(2,1)`, not `DOUBLE` (measured live against Trino
/// 481) — `DOUBLE '…'` is unambiguous and Rust's `{f}` (shortest round-trip
/// `Display`) is exact because a `Param::F64` can never carry NaN/Infinity
/// (JSON, its only realistic origin, cannot express either).
///
/// Every `String`-shaped value (`Str`, and the text payload inside `Decimal`/
/// `Date`/`Timestamp`) goes through [`quote_string`] — the single shared
/// dialect-escaping primitive (see its own doc comment) — never hand-rolled
/// quoting. That is the one property this function exists to guarantee: no
/// other code path in this file is allowed to turn a caller value into SQL
/// text.
fn render_literal(p: &Param) -> String {
    match p {
        Param::Null => "NULL".to_string(),
        Param::Bool(b) => if *b { "true" } else { "false" }.to_string(),
        Param::I64(n) => n.to_string(),
        Param::F64(f) => format!("DOUBLE {}", quote_string(Engine::Trino, &format!("{f}"))),
        Param::Decimal(s) => format!("DECIMAL {}", quote_string(Engine::Trino, s)),
        Param::Str(s) => quote_string(Engine::Trino, s),
        Param::Date(s) => format!("DATE {}", quote_string(Engine::Trino, s)),
        Param::Timestamp(s) => format!("TIMESTAMP {}", quote_string(Engine::Trino, s)),
    }
}

/// Normalize a Trino type name read from `DESCRIBE OUTPUT`'s `"Type"` result
/// column so it matches the spelling `execute`/`describe` report for the
/// *same* type.
///
/// Measured live (Trino 481), same SQL, same logical column: the ordinary
/// `/v1/statement` execute response spells a decimal column's type
/// `"decimal(10, 2)"` (a space after the comma — matches
/// `render_literal`'s own test fixture `"decimal(9, 4)"` and is Trino's
/// `Type.getDisplayName()` convention), while `DESCRIBE OUTPUT`'s `"Type"`
/// result column spells the identical type `"decimal(10,2)"` — no space.
/// `varchar(20)` was measured identical both ways (a single-argument type
/// has no comma to diverge on), so this is specifically a multi-argument
/// type-signature-vs-display-name difference between the two Trino server
/// code paths `describe` and `describe_with_params` go through, not a
/// difference in the underlying type. Left unfixed, a parameterised
/// endpoint's `decimal` column would carry a different `type` string than
/// an otherwise-identical non-parameterised one, and Task 7's CAST-typing
/// rule would inherit that inconsistency silently.
///
/// The fix normalizes at the boundary rather than trying to special-case
/// `decimal`: insert a single space after every comma that isn't already
/// followed by whitespace. This is a plain byte scan, not a type-grammar
/// parser — safe here because Trino type-signature strings never quote or
/// escape a comma (no string literal can appear inside one), so every `,`
/// byte is a genuine argument separator. Idempotent on already-correctly-
/// spaced input (the execute-response spelling passes through unchanged),
/// so applying it unconditionally to `describe_with_params`'s output never
/// second-guesses a type that was already right.
fn normalize_trino_type_spacing(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        out.push(c);
        if c == ',' && chars.peek().is_some_and(|next| !next.is_whitespace()) {
            out.push(' ');
        }
    }
    out
}

/// Percent-encode `s` for use as (part of) an HTTP header value, per RFC 3986
/// — every byte except the unreserved set (`A-Z a-z 0-9 - _ . ~`) becomes
/// `%XX`. Operates on bytes, not `char`s, so multi-byte UTF-8 encodes
/// correctly. Verified live against Trino 481: it decodes both this
/// (percent) and form (`+`-for-space) encodings, so the stricter, unambiguous
/// percent form is used here rather than relying on that leniency.
///
/// A hand-rolled encoder rather than a new dependency: `url` is already a
/// workspace crate but only as a **dev**-dependency of `egret-query` (used to
/// parse `EGRET_TEST_TRINO_URL` in tests), and its `form_urlencoded` encodes
/// space as `+`, which is the leniency this function deliberately avoids
/// relying on.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub struct TrinoConnector {
    base_url: String,
    /// Trino basic-auth principal — always the connection's service user.
    auth_user: String,
    /// Identity sent as `X-Trino-User`. Equals `auth_user` unless the connection
    /// opts into impersonation, in which case it's the resolved end-user.
    session_user: String,
    secret: Option<String>,
    /// Per-user OIDC engine JWT. When set, sent as `Authorization: Bearer` and
    /// takes precedence over `secret` (basic-auth).
    bearer: Option<String>,
    /// True when the base URL is `https` — the connection is genuinely TLS.
    /// Gates the `X-Forwarded-Proto` insecure-HTTP workaround off.
    secure: bool,
    /// PEM of the coordinator's self-signed cert to pin as a trusted root when
    /// connecting over TLS (jwt-mode). `None` = system roots / plain HTTP.
    tls_cert: Option<String>,
    /// Set (to a refusal message) when this connection carries a per-user JWT but
    /// the transport is not TLS — sending the bearer would leak it in the clear.
    /// Enforced in `execute` (fail closed). `None` = safe or explicit opt-out.
    insecure_bearer: Option<String>,
    /// Session catalog and schema (`X-Trino-Catalog` / `X-Trino-Schema`), so
    /// unqualified names in the statement resolve as they did for its author.
    /// `None` on every connection except the view-lineage EXPLAIN
    /// ([`super::run_lineage_explain`]), which needs a view's default catalog
    /// and namespace.
    session_catalog: Option<String>,
    session_schema: Option<String>,
}

impl TrinoConnector {
    pub fn new(spec: &ConnSpec) -> Self {
        let scheme = spec.options.http_scheme.as_deref().unwrap_or("http");
        let auth_user = spec.options.user.clone().unwrap_or_else(|| "egret".into());
        // When a bearer JWT is set the token already authenticates the end user, so
        // X-Trino-User is the resolved session_user (same as the impersonate path).
        // For the basic-auth path, impersonate only when the connection opts in.
        let session_user = if spec.bearer_token.is_some() || spec.options.impersonate == Some(true)
        {
            spec.session_user
                .clone()
                .unwrap_or_else(|| auth_user.clone())
        } else {
            auth_user.clone()
        };
        let secure = scheme == "https";
        TrinoConnector {
            base_url: format!("{scheme}://{}:{}", spec.host, spec.port),
            auth_user,
            session_user,
            secret: spec.secret.clone(),
            bearer: spec.bearer_token.clone(),
            secure,
            tls_cert: spec.options.tls_cert.clone(),
            // Trino carries the JWT in an `Authorization: Bearer` header — TLS is the
            // `https` transport (pinned cert or system roots both protect it), so the
            // guard keys on `secure` (scheme == https), not on `tls_cert` presence.
            insecure_bearer: super::insecure_bearer_reason(
                spec.bearer_token.is_some(),
                secure,
                super::tls_insecure_optout(),
            ),
            session_catalog: None,
            session_schema: None,
        }
    }

    /// The same connection with a session catalog and, when `schema` is not
    /// empty, a session schema. Trino refuses a schema without a catalog, so a
    /// schema is only ever sent beside one.
    pub(super) fn with_session(mut self, catalog: &str, schema: &str) -> Self {
        self.session_catalog = Some(catalog.to_string());
        self.session_schema = (!schema.is_empty()).then(|| schema.to_string());
        self
    }

    /// This connection's cache key in [`super::http_pool`] — the coordinator's
    /// origin and its pinned PEM, and deliberately nothing else.
    ///
    /// **This is the projection the pool's safety argument rests on.** Every
    /// per-principal value on a `TrinoConnector` — `secret`, `session_user`,
    /// `bearer` — is absent here by construction, so no two principals can be
    /// told apart by the key and therefore none can be *conflated* by it either:
    /// the client they share carries no identity at all, and each request
    /// attaches its own in [`Self::auth`]. See the module comment on
    /// [`super::http_pool`], and
    /// `the_client_key_ignores_every_identity_field` below.
    fn client_key(&self) -> super::http_pool::HttpClientKey {
        super::http_pool::HttpClientKey {
            base_url: self.base_url.clone(),
            tls_cert: self.tls_cert.clone(),
        }
    }

    /// The shared `reqwest::Client` for this coordinator, built on first use.
    ///
    /// Pinning the coordinator's self-signed cert as a trusted root when
    /// `tls_cert` is set (jwt-mode/TLS) supersedes `danger_accept_invalid_certs`;
    /// without a pinned cert the client falls back to system roots (unchanged for
    /// plain-HTTP engines).
    ///
    /// The per-query deadline is **not** on the client any more — it is applied
    /// per request by [`Self::timed`], because one client now serves callers that
    /// pass different budgets.
    fn client(&self) -> Result<reqwest::Client, EngineError> {
        super::http_pool::client_for(&self.client_key())
    }

    /// Apply the caller's deadline to one request.
    ///
    /// Every request this connector builds goes through here. It used to be
    /// `Client::builder().timeout(...)`, which was the same deadline configured
    /// one level up; moving it did not weaken it, but forgetting it on a request
    /// would leave that request unbounded, which is why there is exactly one
    /// spelling of it.
    fn timed(
        req: reqwest::RequestBuilder,
        timeout: std::time::Duration,
    ) -> reqwest::RequestBuilder {
        req.timeout(timeout)
    }

    /// The `X-Trino-User` identity (impersonated end-user, or the service user).
    pub fn x_trino_user(&self) -> &str {
        &self.session_user
    }

    /// The basic-auth principal (always the service user).
    pub fn auth_user(&self) -> &str {
        &self.auth_user
    }

    /// The per-user OIDC engine JWT, if set.
    /// Used in tests and by the kernel query handler (Task 2) to verify plumbing.
    #[allow(dead_code)]
    pub fn bearer(&self) -> Option<&str> {
        self.bearer.as_deref()
    }

    /// The transport-security refusal, if this connection would leak a bearer JWT
    /// over non-TLS. `execute` returns this as a `Connect` error (fail closed).
    #[cfg(test)]
    pub fn insecure_bearer(&self) -> Option<&str> {
        self.insecure_bearer.as_deref()
    }

    /// Attach auth + identity to a Trino request.
    /// Trino ALWAYS requires the session user (`X-Trino-User`) to be sent, even on
    /// an authenticated request — omitting it fails with "X-Trino-User must be
    /// sent". With a bearer JWT the engine authenticates the caller's
    /// `principal-field` (provisioned as `preferred_username`), and `session_user`
    /// is that same username, so sending it as `X-Trino-User` is a self-
    /// impersonation (principal == user) that Ranger allows by default. Without a
    /// bearer, send `X-Trino-User` (+ basic-auth when a secret is set).
    ///
    /// `X-Forwarded-Proto: https` is the insecure-HTTP workaround for the
    /// basic-auth (impersonate) path: egret-provisioned plain-HTTP coordinators run
    /// with `http-server.process-forwarded=true` and reject a PASSWORD credential
    /// unless the request is (or is forwarded as) secure — "Password not allowed
    /// for insecure authentication". egret reaches such a coordinator directly
    /// (Service/port-forward, no TLS-terminating proxy), so it presents the header
    /// itself. It is sent ONLY over plain HTTP: over genuine TLS (`secure`, e.g.
    /// jwt-mode with a pinned cert) the request is already secure, so the header is
    /// redundant and is omitted.
    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let req = req.header("X-Trino-User", self.x_trino_user());
        let req = match &self.session_catalog {
            Some(catalog) => req.header("X-Trino-Catalog", catalog),
            None => req,
        };
        let req = match &self.session_schema {
            Some(schema) => req.header("X-Trino-Schema", schema),
            None => req,
        };
        // Only spoof forwarded-https over plain HTTP: it's the insecure-auth
        // workaround. Over genuine TLS the request already is secure, so the
        // header is redundant (and misleading on a non-forwarding target).
        let req = if self.secure {
            req
        } else {
            req.header("X-Forwarded-Proto", "https")
        };
        if let Some(jwt) = &self.bearer {
            req.bearer_auth(jwt)
        } else {
            match &self.secret {
                Some(s) => req.basic_auth(self.auth_user(), Some(s)),
                None => req,
            }
        }
    }

    /// Rewrite an absolute Trino `nextUri` onto this connection's `base_url`,
    /// keeping only its path + query. The coordinator builds the nextUri from the
    /// (forwarded) request scheme/host, which need not match how egret reaches it.
    fn rebase(&self, uri: &str) -> String {
        let path = match uri.split_once("://") {
            Some((_scheme, authority_and_path)) => authority_and_path
                .find('/')
                .map_or("/", |i| &authority_and_path[i..]),
            None => uri, // already a relative path
        };
        format!("{}{}", self.base_url, path)
    }
}

#[derive(Deserialize)]
struct TrinoResp {
    #[serde(default, rename = "nextUri")]
    next_uri: Option<String>,
    #[serde(default)]
    error: Option<TrinoError>,
    #[serde(default)]
    columns: Option<Vec<TrinoColumn>>,
    #[serde(default)]
    data: Option<Vec<Vec<serde_json::Value>>>,
    #[serde(default)]
    stats: Option<TrinoStats>,
}
#[derive(Deserialize)]
struct TrinoColumn {
    name: String,
    #[serde(rename = "type")]
    ty: String,
}
#[derive(Deserialize)]
struct TrinoError {
    message: String,
}
#[derive(Deserialize)]
struct TrinoStats {
    #[serde(default, rename = "processedBytes")]
    processed_bytes: Option<u64>,
}

impl TrinoConnector {
    /// Drive one Trino statement (already POSTed) through the `nextUri`
    /// pagination loop to a `TabularResult`, honoring the row cap and
    /// cooperative cancellation. Shared by `execute` and `execute_with_params`
    /// — they differ only in how the initial POST is built (raw SQL body vs.
    /// `EXECUTE … USING …` body + the prepared-statement header); once the
    /// first response is in hand, the pagination/cap/cancel behavior is
    /// identical, and it matters that it stays that way rather than the two
    /// paths quietly diverging (e.g. one forgetting the row cap or the
    /// mid-page DELETE-on-cancel).
    ///
    /// Measured (spec §3.1): the `X-Trino-Prepared-Statement` header is
    /// required only on the initial POST — the `nextUri` GET loop below never
    /// sends it, verified across a 200 000-row, 10-hop live fetch.
    ///
    /// `retryable` applies to the FIRST request only — the statement POST, which
    /// is the one that can execute something. Every `nextUri` poll below is a
    /// read of a query that already exists and is retried unconditionally. See
    /// [`send`].
    async fn run_statement(
        &self,
        client: &reqwest::Client,
        req: reqwest::RequestBuilder,
        retryable: Retryable,
        limits: &Limits,
        cancel: CancellationToken,
    ) -> Result<TabularResult, EngineError> {
        let cap = limits.max_rows;
        let mut columns: Vec<ColumnMeta> = Vec::new();
        let mut rows: Vec<Vec<serde_json::Value>> = Vec::new();
        let mut bytes_scanned: Option<u64> = None;

        let mut resp: TrinoResp = send(req, retryable).await?;
        loop {
            if let Some(err) = resp.error.take() {
                return Err(EngineError::Engine(err.message));
            }
            if columns.is_empty() {
                if let Some(cols) = resp.columns.take() {
                    columns = cols
                        .into_iter()
                        .map(|c| ColumnMeta {
                            name: c.name,
                            ty: c.ty,
                        })
                        .collect();
                }
            }
            if let Some(st) = &resp.stats {
                if st.processed_bytes.is_some() {
                    bytes_scanned = st.processed_bytes;
                }
            }
            if let Some(data) = resp.data.take() {
                for r in data {
                    if rows.len() <= cap {
                        rows.push(r);
                    }
                    if rows.len() > cap {
                        break;
                    }
                }
            }
            // Follow the paginated `nextUri` at OUR base scheme+host, not the
            // absolute URL Trino returns. Because we send `X-Forwarded-Proto: https`
            // the coordinator echoes `https://…` into the nextUri, but we reach it
            // directly (e.g. plain-HTTP Service), so honor only its path+query.
            let next = resp.next_uri.take().map(|u| self.rebase(&u));
            if cancel.is_cancelled() {
                if let Some(uri) = &next {
                    let _ = Self::timed(self.auth(client.delete(uri)), limits.timeout)
                        .send()
                        .await;
                }
                return Err(EngineError::Cancelled);
            }
            if rows.len() > cap {
                if let Some(uri) = &next {
                    let _ = Self::timed(self.auth(client.delete(uri)), limits.timeout)
                        .send()
                        .await;
                }
                break;
            }
            match next {
                None => break,
                Some(uri) => {
                    // A poll, never an execution: always retryable.
                    resp = send(
                        Self::timed(self.auth(client.get(&uri)), limits.timeout),
                        Retryable::Yes,
                    )
                    .await?;
                }
            }
        }

        let truncated = rows.len() > cap;
        if truncated {
            rows.truncate(cap);
        }
        Ok(TabularResult {
            columns,
            row_count: rows.len() as u64,
            rows,
            bytes_scanned,
            truncated,
        })
    }
}

#[async_trait]
impl Connector for TrinoConnector {
    async fn execute(
        &self,
        sql: &str,
        limits: &Limits,
        cancel: CancellationToken,
    ) -> Result<TabularResult, EngineError> {
        if let Some(reason) = &self.insecure_bearer {
            return Err(EngineError::Connect(reason.clone()));
        }
        let client = self.client()?;

        let session = format!("query_max_run_time={}s", limits.timeout.as_secs().max(1));
        let req = Self::timed(
            self.auth(
                client
                    .post(format!("{}/v1/statement", self.base_url))
                    .header("X-Trino-Session", session)
                    .body(sql.to_string()),
            ),
            limits.timeout,
        );

        // The only call site where the SQL is arbitrary: `job.rs`'s `run_query`
        // hands SQL Lab and notebook text straight to this method, so what a 503
        // here may re-execute is decided from the statement itself.
        let retryable = if is_read_statement(sql) {
            Retryable::Yes
        } else {
            Retryable::No
        };
        self.run_statement(&client, req, retryable, limits, cancel)
            .await
    }

    /// Execute `sql` as a Trino prepared statement, rendering `params` as the
    /// `EXECUTE <name> USING <literal>, <literal>, …` list. See the module
    /// doc comment: the `USING` list is SQL text this connector renders, not
    /// a wire-level bind — [`render_literal`] and `quote_string` are the
    /// entire injection boundary.
    ///
    /// `sql` travels in the `X-Trino-Prepared-Statement: <name>=<percent-
    /// encoded sql>` header (required only on this POST, per
    /// `run_statement`'s doc comment); a 120 024-byte statement measured 200
    /// live, so header size is not a practical constraint.
    ///
    /// **Not yet timeout-bound beyond what `execute` already provides** —
    /// same caveat as the StarRocks binding path (Task 2): `limits.timeout`
    /// governs the `reqwest` client's per-request timeout exactly as it does
    /// for `execute`, but there is no additional wrapper here. Whoever wires
    /// the Data API's serving path to this method inherits the same
    /// outer-timeout obligation already noted on `execute_as`.
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
        if let Some(reason) = &self.insecure_bearer {
            return Err(EngineError::Connect(reason.clone()));
        }
        let client = self.client()?;
        let req = self.prepared_request(&client, sql, params, limits);

        // `sql` is the prepared statement's text (it travels in the
        // `X-Trino-Prepared-Statement` header); the body is `EXECUTE p1 USING …`,
        // which executes it. So the classification is of `sql`, not of the body.
        let retryable = if is_read_statement(sql) {
            Retryable::Yes
        } else {
            Retryable::No
        };
        self.run_statement(&client, req, retryable, limits, cancel)
            .await
    }

    /// Typed columns of a parameterised `SELECT`, without supplying values.
    ///
    /// Trino has no equivalent of a raw `DESCRIBE`-without-executing — the
    /// only way to learn a query's output schema without running it is
    /// `DESCRIBE OUTPUT <name>` against a *prepared* statement. So this
    /// registers the wrapped query as a header-scoped prepared statement
    /// (same `X-Trino-Prepared-Statement: <name>=<percent-encoded sql>`
    /// mechanism `execute_with_params` uses — one request, no persisted
    /// session) and sends `DESCRIBE OUTPUT <name>` as the body. Measured
    /// live: this returns the full output schema and no data rows from the
    /// underlying query — e.g. for `SELECT ? AS v, k, d FROM …`, the
    /// response rows are `["v","","","","unknown",1,false]`,
    /// `["k","","","","integer",4,false]`, `["d","","","","date",4,false]`
    /// — column 0 is the output column's name, column 4 is its Trino type
    /// name. Neither is positionally assumed here: both are located by
    /// `DESCRIBE OUTPUT`'s own response column names (`"Column Name"`,
    /// `"Type"`), which is what the response's `columns` metadata actually
    /// declares, rather than trusting today's column order to hold forever.
    ///
    /// A bare `?` in the projection types as `"unknown"`, confirmed by the
    /// example above (`v`) — neither engine infers a parameter's type from
    /// context, which is the constraint `describe_with_params_types_a_bare_placeholder_as_unknown_but_a_cast_fixes_it`
    /// pins for Task 7's `CAST` rule.
    ///
    /// Every type string is passed through [`normalize_trino_type_spacing`]
    /// before being returned — see its doc comment for the measured
    /// `decimal(10,2)` vs `decimal(10, 2)` divergence between this path and
    /// `describe`'s, which `describe_and_describe_with_params_type_the_same_columns_identically_on_trino`
    /// in the test file pins as the two paths' outputs matching exactly for
    /// shared columns.
    async fn describe_with_params(
        &self,
        sql: &str,
        param_count: usize,
        timeout: std::time::Duration,
    ) -> Result<Vec<ColumnMeta>, EngineError> {
        if param_count == 0 {
            return self.describe(sql, timeout).await;
        }
        if let Some(reason) = &self.insecure_bearer {
            return Err(EngineError::Connect(reason.clone()));
        }
        let client = self.client()?;
        let wrapped = format!("SELECT * FROM (\n{sql}\n) AS _egret_ds LIMIT 0");
        let name = "p1";
        let prepared_header = format!("{name}={}", percent_encode(&wrapped));
        let session = format!("query_max_run_time={}s", timeout.as_secs().max(1));
        let req = Self::timed(
            self.auth(
                client
                    .post(format!("{}/v1/statement", self.base_url))
                    .header("X-Trino-Session", session)
                    .header("X-Trino-Prepared-Statement", prepared_header)
                    .body(format!("DESCRIBE OUTPUT {name}")),
            ),
            timeout,
        );
        // DESCRIBE OUTPUT's own result has one row per output column of the
        // wrapped query, never per-row query data — a generous cap costs
        // nothing here (unlike `execute`, where it bounds real result size).
        let limits = Limits {
            max_rows: 100_000,
            timeout,
        };
        // The body is `DESCRIBE OUTPUT p1`: it reads a prepared statement's
        // output schema and runs nothing, so re-sending it cannot execute the
        // wrapped query even once.
        let result = self
            .run_statement(
                &client,
                req,
                Retryable::Yes,
                &limits,
                CancellationToken::new(),
            )
            .await?;

        let name_idx = result
            .columns
            .iter()
            .position(|c| c.name == "Column Name")
            .ok_or_else(|| {
                EngineError::Engine("DESCRIBE OUTPUT response has no \"Column Name\" column".into())
            })?;
        let type_idx = result
            .columns
            .iter()
            .position(|c| c.name == "Type")
            .ok_or_else(|| {
                EngineError::Engine("DESCRIBE OUTPUT response has no \"Type\" column".into())
            })?;

        Ok(result
            .rows
            .into_iter()
            .map(|row| ColumnMeta {
                name: row[name_idx].as_str().unwrap_or_default().to_string(),
                ty: normalize_trino_type_spacing(row[type_idx].as_str().unwrap_or_default()),
            })
            .collect())
    }
}

impl TrinoConnector {
    /// Build the (unsent) POST request for a prepared-statement execution —
    /// the `X-Trino-Prepared-Statement` header and the `EXECUTE p1 USING
    /// <literal>, …` body. Split out of `execute_with_params` so its exact
    /// shape can be pinned by an OFFLINE test: with `EGRET_TEST_TRINO_URL`
    /// unset, `connector_trino_params.rs` self-skips entirely, so nothing in
    /// CI would otherwise notice a wrong header name or a dropped
    /// `percent_encode` call.
    fn prepared_request(
        &self,
        client: &reqwest::Client,
        sql: &str,
        params: &[Param],
        limits: &Limits,
    ) -> reqwest::RequestBuilder {
        let session = format!("query_max_run_time={}s", limits.timeout.as_secs().max(1));
        let name = "p1";
        let using = params
            .iter()
            .map(render_literal)
            .collect::<Vec<_>>()
            .join(", ");
        let body = format!("EXECUTE {name} USING {using}");
        let prepared_header = format!("{name}={}", percent_encode(sql));
        Self::timed(
            self.auth(
                client
                    .post(format!("{}/v1/statement", self.base_url))
                    .header("X-Trino-Session", session)
                    .header("X-Trino-Prepared-Statement", prepared_header)
                    .body(body),
            ),
            limits.timeout,
        )
    }
}

/// How many times one Trino request is sent before a 503 is reported as a
/// failure — the initial attempt plus four retries.
///
/// Bounded on purpose. The Trino client protocol says to keep retrying a 503,
/// and its own Java client does so for up to two minutes; egret cannot, because
/// every caller here already sits under a wall-clock budget it did not choose
/// (`Limits::timeout`, `DESCRIBE_TIMEOUT`, the JobManager's). Retrying past that
/// budget cannot succeed — the outer `tokio::time::timeout` drops this future —
/// so the only thing an unbounded loop would add is a request the caller is no
/// longer waiting for. Five attempts with the backoff below cost at most
/// [`TRINO_RETRY_MAX_TOTAL_WAIT`] of sleeping, which fits inside every budget in
/// the system.
const TRINO_RETRY_MAX_ATTEMPTS: u32 = 5;

/// First backoff, doubled per retry and capped: 50ms, 100ms, 200ms, 400ms.
///
/// The Trino protocol documents ~50-100ms as the right initial interval for a
/// coordinator that is busy rather than broken.
const TRINO_RETRY_BASE_BACKOFF: std::time::Duration = std::time::Duration::from_millis(50);

/// Ceiling on ONE sleep, whether it came from the backoff or from `Retry-After`.
///
/// `Retry-After` is a value the coordinator chooses, and this connector is the
/// thing on the other end of a request a human or a Data API caller is waiting
/// on. Honouring an arbitrary `Retry-After: 3600` would turn a busy coordinator
/// into a hung egret request. Capping it means egret still *respects* the
/// header — it waits longer than its own backoff would have — without handing
/// the engine control of egret's latency.
const TRINO_RETRY_MAX_BACKOFF: std::time::Duration = std::time::Duration::from_millis(1000);

/// The most time the retries of a single request can spend asleep.
/// Stated as a constant so the bound is checkable rather than inferred from the
/// two above: 50 + 100 + 200 + 400 = 750ms of backoff, and at most
/// 4 x 1000ms if every attempt carries a `Retry-After`.
#[cfg(test)]
const TRINO_RETRY_MAX_TOTAL_WAIT: std::time::Duration = std::time::Duration::from_millis(4000);

/// The `Retry-After` value, as a duration, when the header is present and is a
/// delta-seconds count.
///
/// **Delta-seconds only, not the HTTP-date form.** RFC 9110 permits both, and a
/// date requires a clock comparison against a server whose clock egret does not
/// trust — a coordinator running five minutes fast would produce a negative or
/// absurd wait from a perfectly well-formed header. Trino sends the numeric form
/// when it sends one at all. An unparseable or date-shaped header is treated as
/// absent, which falls back to egret's own backoff: strictly safe, because the
/// fallback is shorter.
fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<std::time::Duration> {
    let raw = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    let secs: u64 = raw.trim().parse().ok()?;
    Some(std::time::Duration::from_secs(secs))
}

/// The outcome of ONE attempt, separating "retry this" from every other answer
/// so [`send`] can decide without re-inspecting a consumed response.
enum Attempt {
    /// A final answer — success or a failure that retrying cannot change.
    Done(Result<TrinoResp, EngineError>),
    /// The coordinator said 503. Per the Trino client protocol the query is
    /// still alive and the same URI should be re-requested.
    Retry(Option<std::time::Duration>),
}

async fn send_once(req: reqwest::RequestBuilder) -> Attempt {
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => return Attempt::Done(Err(EngineError::Connect(e.to_string()))),
    };
    if !resp.status().is_success() {
        let status = resp.status();
        // **503 is not a failure in this protocol.** The Trino client protocol
        // defines a 503 on `POST /v1/statement` and on a `nextUri` GET as
        // "the coordinator is busy, the query is still alive, send the same
        // request again". Reporting it as an engine error abandoned a query
        // that was still running and answered the caller with a 500 —
        // egret's own bug reported as the engine's.
        if status == reqwest::StatusCode::SERVICE_UNAVAILABLE {
            return Attempt::Retry(parse_retry_after(resp.headers()));
        }
        // Per-user delegation. 403 is the engine denying an authenticated user
        // (Ranger/file-ACL deny): a clean Forbidden. 401 is the engine failing
        // to AUTHENTICATE egret's JWT (expired, bad signature, empty JWKS) —
        // not an access denial, so it gets its own variant and its reason is
        // logged. Only the engine's response body is logged, never the request.
        if status == reqwest::StatusCode::FORBIDDEN {
            return Attempt::Done(Err(EngineError::Forbidden));
        }
        if status == reqwest::StatusCode::UNAUTHORIZED {
            let www = resp
                .headers()
                .get(reqwest::header::WWW_AUTHENTICATE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            let body = resp.text().await.unwrap_or_default();
            let reason: String = body.chars().take(300).collect();
            tracing::warn!(
                %status,
                www_authenticate = %www,
                reason = %reason,
                "trino rejected egret's credentials (engine authentication failed)"
            );
            return Attempt::Done(Err(EngineError::AuthFailed(reason)));
        }
        return Attempt::Done(Err(EngineError::Engine(format!("http {status}"))));
    }
    Attempt::Done(
        resp.json::<TrinoResp>()
            .await
            .map_err(|e| EngineError::Engine(format!("decode: {e}"))),
    )
}

/// May a 503 on THIS request be re-sent?
///
/// The distinction exists because the two requests this connector sends are not
/// equally safe to repeat, and the difference is not visible from a
/// `RequestBuilder`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Retryable {
    /// Re-sending cannot execute anything twice: a `nextUri` poll (a read of a
    /// query that already exists), or a statement POST carrying SQL this
    /// connector could prove is a read.
    Yes,
    /// A statement POST carrying SQL that may change data. The 503 is reported
    /// as the error it is, which is what happened to every request before this
    /// package.
    No,
}

/// Is `sql` a statement that running twice is indistinguishable from running
/// once?
///
/// **Deliberately narrow, and conservative in the safe direction.** Anything it
/// cannot prove is a read gets [`Retryable::No`], which costs a retry that might
/// have succeeded — the behaviour every request had before this package — while
/// the opposite mistake would re-execute an `INSERT`. Leading comments are not
/// stripped, for the same reason: `-- note` followed by a `SELECT` classifies as
/// not-a-read and merely loses its retry.
///
/// A `;` anywhere but at the very end disqualifies the statement too. Trino
/// accepts one statement per POST, so a multi-statement body is already a
/// rejection; the check is here so that reading the first keyword cannot stand
/// in for reading the whole string.
///
/// # The one direction this function may never move in
///
/// A statement may LOSE a retry it could have had. A statement may never GAIN
/// one. Every rule below is written so that the failure mode of a wrong guess is
/// a wasted 503, not a duplicated row — because the second mistake is
/// unrecoverable and invisible.
///
/// `EXPLAIN` is where that rule was broken, and the reason is worth keeping:
/// **`EXPLAIN ANALYZE <stmt>` executes `<stmt>`.** It is not a planner verb with
/// an option; it is the statement itself, run, with timings attached. So
/// `EXPLAIN ANALYZE INSERT INTO t SELECT …` read as a leading `explain`,
/// classified as a provable read, and an ingress-synthesised 503 would have
/// re-POSTed it up to [`TRINO_RETRY_MAX_ATTEMPTS`] times — the exact duplicated
/// write the classifier exists to prevent, arriving through the one keyword
/// that was allowed to stand alone.
///
/// The fix is not a special case for `ANALYZE`. `EXPLAIN` is now accepted only
/// when **what it explains is itself provably a read**, checked by the same
/// function. That refuses `EXPLAIN ANALYZE …` (its operand starts with
/// `analyze`, which proves nothing), refuses `EXPLAIN INSERT …`, and refuses
/// `EXPLAIN (TYPE LOGICAL) SELECT 1` too — the last of those is genuinely
/// read-only and merely loses its retry, which is the affordable half of the
/// trade.
///
/// # This is a keyword check, not a parser
///
/// So the allow-list is the whole of its knowledge, and anything outside it is
/// refused rather than reasoned about. Statements deliberately kept OUT even
/// though they can look read-shaped:
///
/// * `EXECUTE <name>` — runs whatever was `PREPARE`d, which may be an `INSERT`.
///   The text at hand cannot say which.
/// * `ANALYZE <table>` — reads a table and WRITES statistics to the metastore.
///   Read-shaped by name only.
/// * `CALL <procedure>` — arbitrary side effects.
/// * `SELECT … FROM TABLE(…)` — a polymorphic table function is opaque to a
///   keyword check. It is accepted, and this is the residual risk: no table
///   function shipped with Trino writes, but a connector could add one, and the
///   line to revisit is here rather than in a comment claiming there is none.
fn is_read_statement(sql: &str) -> bool {
    // `EXPLAIN` may wrap a statement; nothing may wrap an `EXPLAIN`. The bound
    // is what stops a crafted `EXPLAIN EXPLAIN EXPLAIN …` from recursing, and
    // costs nothing real: Trino rejects the nesting anyway.
    fn provable_read(sql: &str, depth: u32) -> bool {
        if depth > 1 {
            return false;
        }
        let trimmed = sql.trim().trim_end_matches(';').trim();
        if trimmed.contains(';') {
            return false;
        }
        // A parenthesised query — `(SELECT 1) UNION (SELECT 2)` — is still a
        // query; no statement that writes may be wrapped in parentheses.
        let head = trimmed.trim_start_matches(|c: char| c == '(' || c.is_whitespace());
        let end = head
            .find(|c: char| c.is_whitespace() || c == '(')
            .unwrap_or(head.len());
        let (first, rest) = head.split_at(end);
        match first.to_ascii_lowercase().as_str() {
            "select" | "with" | "show" | "describe" | "desc" => true,
            // Only if its operand is one too — see the doc comment. `rest` is
            // everything after the keyword, so `ANALYZE` and any write verb is
            // judged on its own merits and fails.
            "explain" => provable_read(rest, depth + 1),
            _ => false,
        }
    }
    provable_read(sql, 0)
}

/// The wait before re-sending, given the attempt number (1-based) and whatever
/// the coordinator asked for.
///
/// **Its whole job is the cap**, which is why it is a function rather than two
/// lines inside [`send`]: `Retry-After` is a number the *coordinator* chooses,
/// and honouring it unbounded hands the thing egret is waiting on the power to
/// decide how long egret waits. See [`TRINO_RETRY_MAX_BACKOFF`].
fn retry_wait(attempt: u32, retry_after: Option<std::time::Duration>) -> std::time::Duration {
    match retry_after {
        // The coordinator's own number, under egret's ceiling.
        Some(d) => d.min(TRINO_RETRY_MAX_BACKOFF),
        // `1 << (attempt - 1)` over a u32 attempt bounded by
        // TRINO_RETRY_MAX_ATTEMPTS cannot overflow, and the result is capped
        // anyway.
        None => TRINO_RETRY_BASE_BACKOFF
            .saturating_mul(1u32 << (attempt.saturating_sub(1)))
            .min(TRINO_RETRY_MAX_BACKOFF),
    }
}

/// Send one Trino request, honouring the protocol's 503-means-retry rule.
///
/// # What makes re-sending safe, and where the line is
///
/// A 503 on a `nextUri` GET is a poll that produced nothing; re-issuing the same
/// `nextUri` is how the protocol advances, and it cannot execute anything. That
/// one is unconditionally safe.
///
/// A 503 on `POST /v1/statement` is different, and the reason is that **this
/// connector does not only carry `SELECT`s** — an earlier version of this
/// comment claimed it did, and that claim was false. `execution/job.rs`'s
/// `run_query` sends SQL Lab and notebook SQL straight through
/// `Connector::execute`, and `apply_sql_limit` explicitly leaves non-`SELECT`
/// and multi-statement SQL unchanged, so an `INSERT` or a `CREATE TABLE` does
/// travel this path.
///
/// A coordinator's own 503 is emitted before the statement is registered, so
/// re-POSTing it duplicates nothing — that is why the protocol says to send it
/// again. But egret does not always talk to the coordinator directly: an
/// ingress, a service mesh or any proxy in front of it can synthesise a 503
/// *after* forwarding the POST, and then a re-POST is a second execution. For a
/// read that is a wasted query; for an `INSERT` it is a duplicated row.
///
/// So the retry is bounded to the case where the claim holds: [`Retryable::Yes`]
/// for every poll, and for a statement POST only when [`is_read_statement`]
/// proves the body is a read. Everything else reports the 503 exactly as it did
/// before this package.
///
/// The `DELETE` sent to cancel is deliberately NOT routed through here: it is
/// already best-effort (`let _ =`), and retrying a cancellation while the caller
/// has stopped waiting buys nothing.
///
/// # Bounded, always
///
/// [`TRINO_RETRY_MAX_ATTEMPTS`] attempts, exponential backoff capped by
/// [`TRINO_RETRY_MAX_BACKOFF`], and `Retry-After` honoured under the same cap
/// ([`retry_wait`]). When the attempts run out the caller gets the same
/// `http 503` error it used to get immediately — a permanently unavailable
/// coordinator is still an error, just no longer one produced on the first try.
async fn send(
    req: reqwest::RequestBuilder,
    retryable: Retryable,
) -> Result<TrinoResp, EngineError> {
    let mut req = req;
    let mut attempt: u32 = 1;
    loop {
        // Cloned BEFORE the send, because `send` consumes the builder.
        // `try_clone` returns `None` only for a streaming body, which this
        // connector never builds (every body is a `String`); if one ever
        // appears, the `None` arm below reports the 503 rather than silently
        // retrying something it cannot reproduce.
        let next = req.try_clone();
        match send_once(req).await {
            Attempt::Done(r) => return r,
            Attempt::Retry(retry_after) => {
                // A statement whose re-execution egret cannot rule out is
                // reported, never repeated — see this function's doc comment.
                if retryable == Retryable::No {
                    return Err(EngineError::Engine(
                        "http 503 Service Unavailable".to_string(),
                    ));
                }
                let Some(next_req) = next else {
                    return Err(EngineError::Engine(
                        "http 503 Service Unavailable".to_string(),
                    ));
                };
                if attempt >= TRINO_RETRY_MAX_ATTEMPTS {
                    return Err(EngineError::Engine(
                        "http 503 Service Unavailable".to_string(),
                    ));
                }
                let wait = retry_wait(attempt, retry_after);
                tracing::debug!(
                    attempt,
                    wait_ms = wait.as_millis() as u64,
                    honoured_retry_after = retry_after.is_some(),
                    "trino coordinator returned 503; the query is still alive, retrying"
                );
                tokio::time::sleep(wait).await;
                attempt += 1;
                req = next_req;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::model::{ConnOptions, Engine};

    // --- render_literal (Step 1: no engine needed) ---

    #[test]
    fn null_renders_bare_null() {
        assert_eq!(render_literal(&Param::Null), "NULL");
    }

    #[test]
    fn bool_renders_bare_lowercase() {
        assert_eq!(render_literal(&Param::Bool(true)), "true");
        assert_eq!(render_literal(&Param::Bool(false)), "false");
    }

    #[test]
    fn i64_renders_bare_and_preserves_full_precision() {
        // 2^53 + 1 — the classic float-precision boundary. A bare integer
        // literal, never routed through a lossy f64 conversion.
        assert_eq!(
            render_literal(&Param::I64(9_007_199_254_740_993)),
            "9007199254740993"
        );
        assert_eq!(render_literal(&Param::I64(-42)), "-42");
    }

    #[test]
    fn f64_renders_as_typed_double_literal() {
        // Bare `1.5` is Trino's DECIMAL(2,1), not DOUBLE (measured live) — the
        // typed literal is what pins the value's type unambiguously.
        assert_eq!(render_literal(&Param::F64(1.5)), "DOUBLE '1.5'");
    }

    #[test]
    fn decimal_date_timestamp_render_as_typed_literals() {
        assert_eq!(
            render_literal(&Param::Decimal("123.456".into())),
            "DECIMAL '123.456'"
        );
        assert_eq!(
            render_literal(&Param::Date("2020-02-29".into())),
            "DATE '2020-02-29'"
        );
        assert_eq!(
            render_literal(&Param::Timestamp("2020-02-29 13:14:15.123".into())),
            "TIMESTAMP '2020-02-29 13:14:15.123'"
        );
    }

    // --- normalize_trino_type_spacing (fix round 1: DESCRIBE OUTPUT vs execute) ---

    #[test]
    fn normalize_inserts_a_space_after_an_unspaced_comma() {
        // The exact measured divergence: DESCRIBE OUTPUT spells this
        // "decimal(10,2)"; execute spells it "decimal(10, 2)".
        assert_eq!(
            normalize_trino_type_spacing("decimal(10,2)"),
            "decimal(10, 2)"
        );
    }

    #[test]
    fn normalize_is_a_no_op_on_already_spaced_input() {
        // Idempotent: running it on execute's own spelling must not double
        // the space or otherwise change it.
        assert_eq!(
            normalize_trino_type_spacing("decimal(10, 2)"),
            "decimal(10, 2)"
        );
    }

    #[test]
    fn normalize_leaves_a_comma_free_type_unchanged() {
        assert_eq!(normalize_trino_type_spacing("varchar(20)"), "varchar(20)");
        assert_eq!(normalize_trino_type_spacing("unknown"), "unknown");
        assert_eq!(normalize_trino_type_spacing("integer"), "integer");
    }

    #[test]
    fn normalize_handles_multiple_and_trailing_commas() {
        assert_eq!(
            normalize_trino_type_spacing("row(a integer,b varchar)"),
            "row(a integer, b varchar)"
        );
        // A trailing comma (degenerate, but must not panic/index-out-of-bounds
        // on `chars.peek()` returning `None`).
        assert_eq!(normalize_trino_type_spacing("a,"), "a,");
    }

    #[test]
    fn str_goes_through_quote_string() {
        assert_eq!(render_literal(&Param::Str("hello".into())), "'hello'");
        // "007" must stay a quoted string, not collapse to a bare 7 — this is
        // exactly the string-coerced-to-number failure mode the round-trip
        // test also guards, caught here at the render layer with no engine.
        assert_eq!(render_literal(&Param::Str("007".into())), "'007'");
    }

    /// The property that matters most in this file: an embedded quote is
    /// DOUBLED, never backslash-escaped, so the literal stays closed. This is
    /// the exact behavior `quote_string(Engine::Trino, …)` provides, and
    /// `render_literal` must not roll its own.
    #[test]
    fn embedded_quote_is_doubled_not_backslash_escaped() {
        assert_eq!(render_literal(&Param::Str("O'Brien".into())), "'O''Brien'");
        // The hostile canary this file's live test binds: a naive escaper
        // that forgets doubling would let this break out of the literal.
        assert_eq!(
            render_literal(&Param::Str("'; DROP TABLE zz --".into())),
            "'''; DROP TABLE zz --'"
        );
    }

    /// Trino does not treat `\` as a string-literal escape character
    /// (measured: `USING 'a\b'` is 4 chars, not 2) — so unlike StarRocks,
    /// `render_literal` must NEVER double a backslash. Doubling here would
    /// silently corrupt every Windows path or regex value passed as a filter.
    #[test]
    fn backslash_is_never_doubled() {
        assert_eq!(
            render_literal(&Param::Str(r"C:\Users\bob".into())),
            r"'C:\Users\bob'"
        );
    }

    // --- typed-variant hostile content (fix round 1, Important 2) ---
    //
    // `Decimal`/`Date`/`Timestamp` carry unvalidated caller text through
    // `format!("<TYPE> {}", quote_string(...))`. Nothing upstream enforces
    // that a `Date` actually looks like `YYYY-MM-DD` — so the property that
    // matters is not "the content is well-formed" but "the content, however
    // malformed, still goes through `quote_string` and stays inside ONE
    // typed literal". These three pin the EXACT escaped output (not just
    // "no panic"), so a future rewrite to `format!("DATE '{s}'")` — reasoning
    // that a date can't contain a quote — fails here immediately, offline,
    // rather than only being caught by the live containment tests in
    // `connector_trino_params.rs`.

    /// Reviewer-verified live (in the parenthesised form `"1) OR (1=1"`):
    /// Trino rejects hostile `DECIMAL` content as `not a valid DECIMAL
    /// literal`. That payload contains no `'`, so it happens to render
    /// identically whether or not `quote_string` doubles quotes — it cannot
    /// tell escaping-correct from escaping-broken apart. This variant adds
    /// an embedded quote (mirroring the `Date`/`Timestamp` payloads below)
    /// specifically so the mutation in `connector_trino_params.rs`'s live
    /// twin of this test is meaningful: measured to fail differently
    /// (`TYPE_MISMATCH: Logical expression term must evaluate to a
    /// boolean`) once quote-doubling is removed.
    #[test]
    fn decimal_hostile_content_still_goes_through_quote_string() {
        assert_eq!(
            render_literal(&Param::Decimal("1' OR '1'='1".into())),
            "DECIMAL '1'' OR ''1''=''1'"
        );
    }

    #[test]
    fn date_hostile_content_still_goes_through_quote_string() {
        assert_eq!(
            render_literal(&Param::Date("2020-01-01' OR '1'='1".into())),
            "DATE '2020-01-01'' OR ''1''=''1'"
        );
    }

    /// The exact shape of the attack the reviewer constructed: a naive
    /// `format!("TIMESTAMP '{s}'")` would let this ONE param expand into
    /// THREE `USING` items (a timestamp literal, a bare `current_user`
    /// expression, and a second timestamp literal), evaluating
    /// `current_user` and shifting every subsequent bind position. Routed
    /// through `quote_string`, the embedded quotes are doubled and the whole
    /// string stays inside a single `TIMESTAMP '…'` token.
    #[test]
    fn timestamp_hostile_content_still_goes_through_quote_string() {
        assert_eq!(
            render_literal(&Param::Timestamp(
                "2020-01-01 00:00:00', current_user, TIMESTAMP '2020-01-01 00:00:00".into()
            )),
            "TIMESTAMP '2020-01-01 00:00:00'', current_user, TIMESTAMP ''2020-01-01 00:00:00'"
        );
    }

    // --- percent_encode (Step 2 plumbing) ---

    #[test]
    fn percent_encode_leaves_unreserved_bytes_alone() {
        assert_eq!(percent_encode("abcXYZ019-_.~"), "abcXYZ019-_.~");
    }

    #[test]
    fn percent_encode_escapes_space_and_sql_punctuation() {
        assert_eq!(percent_encode("SELECT ? AS s"), "SELECT%20%3F%20AS%20s");
        // '%' itself must be escaped, or a literal `%` in SQL text (e.g. a
        // LIKE pattern) would corrupt the encoding on decode.
        assert_eq!(percent_encode("a%b"), "a%25b");
    }

    #[test]
    fn percent_encode_is_byte_level_for_multibyte_utf8() {
        // "日" is 3 UTF-8 bytes (E6 97 A5); each must become its own %XX, not
        // one escape per `char`.
        assert_eq!(percent_encode("日"), "%E6%97%A5");
    }

    /// The realistic shape of a Data API template: multi-line SQL with `=`
    /// and `,` in it. `=` and `,` matter specifically — Trino's own header
    /// value is `name=sql`, and a literal (unencoded) `=` or `,` reaching
    /// that header would be ambiguous with the name/value separator or a
    /// future multi-statement extension of it. CR/LF must never survive
    /// either: a bare newline in a header value is a request-splitting
    /// vector, and `reqwest::HeaderValue` refuses to construct one — so if
    /// this encoder ever regressed to passing raw control bytes through,
    /// `.build()` on the actual request would panic/error rather than
    /// silently mis-behave, but this test catches it at the string level
    /// first.
    #[test]
    fn percent_encode_handles_a_multiline_statement_with_cr_lf_and_punctuation() {
        let sql = "SELECT ?\r\nFROM t\nWHERE x = ?,\ty";
        let enc = percent_encode(sql);
        assert_eq!(
            enc,
            "SELECT%20%3F%0D%0AFROM%20t%0AWHERE%20x%20%3D%20%3F%2C%09y"
        );
        assert!(
            !enc.contains(['\r', '\n', '=', ',']),
            "no raw control byte or header-structural character may survive: {enc:?}"
        );
    }

    /// J-F2: an engine 401 is a login failure, a 403 a denial; the 401 carries
    /// the engine's reason.
    #[tokio::test]
    async fn engine_401_is_auth_failed_and_403_is_forbidden() {
        use axum::{http::StatusCode, routing::get, Router};
        let app = Router::new()
            .route(
                "/u",
                get(|| async { (StatusCode::UNAUTHORIZED, "Unable to verify JWT") }),
            )
            .route("/f", get(|| async { (StatusCode::FORBIDDEN, "denied") }));
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let srv = tokio::spawn(async move { axum::serve(l, app).await });
        let client = reqwest::Client::new();
        let r401 = send_once(client.get(format!("http://{addr}/u"))).await;
        let r403 = send_once(client.get(format!("http://{addr}/f"))).await;
        srv.abort();
        match r401 {
            Attempt::Done(Err(EngineError::AuthFailed(why))) => {
                assert!(why.contains("Unable to verify JWT"), "{why}")
            }
            _ => panic!("401 must be AuthFailed"),
        }
        assert!(matches!(r403, Attempt::Done(Err(EngineError::Forbidden))));
    }

    // --- offline request shape (fix round 1, Minor 2) ---
    //
    // With `EGRET_TEST_TRINO_URL` unset, every test in
    // `connector_trino_params.rs` self-skips — so nothing in a normal CI run
    // exercises `execute_with_params`'s actual header/body construction. This
    // test needs no network call: `prepared_request(..).build()` materializes
    // the `reqwest::Request` without sending it, the same pattern
    // `bearer_path_sends_authorization_and_xtrino_user` already uses below.
    #[test]
    fn execute_with_params_emits_the_prepared_statement_header_and_execute_using_body() {
        let c = TrinoConnector::new(&ConnSpec {
            engine: Engine::Trino,
            host: "h".into(),
            port: 8080,
            options: ConnOptions::default(),
            secret: None,
            session_user: Some("alice".into()),
            bearer_token: None,
        });
        let client = reqwest::Client::new();
        let limits = Limits {
            max_rows: 100,
            timeout: std::time::Duration::from_secs(30),
        };
        let req = c
            .prepared_request(
                &client,
                "SELECT ? AS s, ? AS n",
                &[Param::Str("hi".into()), Param::I64(42)],
                &limits,
            )
            .build()
            .unwrap();

        assert_eq!(req.method(), reqwest::Method::POST);
        assert_eq!(req.url().as_str(), "http://h:8080/v1/statement");

        let h = req.headers();
        assert_eq!(
            h.get("X-Trino-Prepared-Statement").unwrap(),
            "p1=SELECT%20%3F%20AS%20s%2C%20%3F%20AS%20n",
            "the header name must be `p1`, matching the `EXECUTE p1 …` body below, \
             and the SQL must be percent-encoded, not raw"
        );

        let body_bytes = req
            .body()
            .expect("execute_with_params always sets a body")
            .as_bytes()
            .expect("body was built from a String, so it's fully buffered");
        assert_eq!(
            std::str::from_utf8(body_bytes).unwrap(),
            "EXECUTE p1 USING 'hi', 42",
            "the USING list must be rendered literals, comma-joined, matching render_literal"
        );
    }

    #[test]
    fn xtrino_user_is_session_user_only_when_impersonating() {
        let mut opts = ConnOptions {
            user: Some("svc".into()),
            ..Default::default()
        };
        // impersonate off → X-Trino-User = service user (even if session_user present)
        let s0 = ConnSpec {
            engine: Engine::Trino,
            host: "h".into(),
            port: 8080,
            options: opts.clone(),
            secret: None,
            session_user: Some("alice".into()),
            bearer_token: None,
        };
        assert_eq!(TrinoConnector::new(&s0).x_trino_user(), "svc");

        // impersonate on + session_user → X-Trino-User = alice; basic-auth stays svc
        opts.impersonate = Some(true);
        let s1 = ConnSpec {
            engine: Engine::Trino,
            host: "h".into(),
            port: 8080,
            options: opts,
            secret: None,
            session_user: Some("alice".into()),
            bearer_token: None,
        };
        let c = TrinoConnector::new(&s1);
        assert_eq!(c.x_trino_user(), "alice");
        assert_eq!(c.auth_user(), "svc");
    }

    #[test]
    fn bearer_token_takes_precedence_over_secret() {
        let s = ConnSpec {
            engine: Engine::Trino,
            host: "h".into(),
            port: 8080,
            options: ConnOptions {
                user: Some("svc".into()),
                ..Default::default()
            },
            secret: Some("basic-pw".into()),
            session_user: Some("alice".into()),
            bearer_token: Some("eyJ.jwt.sig".into()),
        };
        let c = TrinoConnector::new(&s);
        assert_eq!(c.bearer(), Some("eyJ.jwt.sig"));
        assert_eq!(c.x_trino_user(), "alice");
    }

    #[test]
    fn no_bearer_uses_basic_auth_path() {
        let s = ConnSpec {
            engine: Engine::Trino,
            host: "h".into(),
            port: 8080,
            options: ConnOptions::default(),
            secret: Some("pw".into()),
            session_user: None,
            bearer_token: None,
        };
        assert_eq!(TrinoConnector::new(&s).bearer(), None);
    }

    // A per-user JWT over plain HTTP would leak the token — the connection is
    // flagged insecure so `execute` fails closed. Over `https` (jwt-mode) it's fine.
    // No bearer (impersonate / service user) is never flagged, plain HTTP or not.
    #[test]
    fn bearer_requires_tls() {
        let mk = |scheme: Option<&str>, bearer: Option<&str>| {
            TrinoConnector::new(&ConnSpec {
                engine: Engine::Trino,
                host: "h".into(),
                port: 8080,
                options: ConnOptions {
                    http_scheme: scheme.map(str::to_string),
                    ..Default::default()
                },
                secret: None,
                session_user: Some("alice".into()),
                bearer_token: bearer.map(str::to_string),
            })
        };
        // bearer over plain http → refused
        assert!(mk(None, Some("eyJ.jwt.sig")).insecure_bearer().is_some());
        assert!(mk(Some("http"), Some("eyJ.jwt.sig"))
            .insecure_bearer()
            .is_some());
        // bearer over https → allowed
        assert!(mk(Some("https"), Some("eyJ.jwt.sig"))
            .insecure_bearer()
            .is_none());
        // no bearer → never flagged, even over plain http
        assert!(mk(None, None).insecure_bearer().is_none());
    }

    // The emitted headers — not just the accessors — are what Trino sees. Trino
    // rejects an authenticated request that omits X-Trino-User ("X-Trino-User must
    // be sent"), so the bearer path MUST send BOTH Authorization: Bearer and
    // X-Trino-User (= the caller's username, a self-impersonation). Regression
    // guard for the delegated-query 401.
    #[test]
    fn bearer_path_sends_authorization_and_xtrino_user() {
        let c = TrinoConnector::new(&ConnSpec {
            engine: Engine::Trino,
            host: "h".into(),
            port: 8080,
            options: ConnOptions::default(),
            secret: Some("basic-pw".into()),
            session_user: Some("alice".into()),
            bearer_token: Some("eyJ.jwt.sig".into()),
        });
        let client = reqwest::Client::new();
        let req = c
            .auth(client.post("http://h:8080/v1/statement"))
            .build()
            .unwrap();
        let h = req.headers();
        assert_eq!(h.get("X-Trino-User").unwrap(), "alice");
        assert_eq!(h.get("authorization").unwrap(), "Bearer eyJ.jwt.sig");
        // Present the connection as forwarded-https so a process-forwarded
        // coordinator accepts auth (required for the basic-auth impersonate path).
        assert_eq!(h.get("X-Forwarded-Proto").unwrap(), "https");
    }

    // View governance B1: the lineage EXPLAIN resolves a view's unqualified names
    // against the view's default catalog and namespace, so they travel as the
    // session catalog and schema. No other connection sends either header.
    #[test]
    fn a_session_catalog_and_schema_are_sent_only_when_set() {
        let spec = ConnSpec {
            engine: Engine::Trino,
            host: "h".into(),
            port: 8080,
            options: ConnOptions::default(),
            secret: None,
            session_user: Some("egret-lineage".into()),
            bearer_token: None,
        };
        let client = reqwest::Client::new();
        let plain = TrinoConnector::new(&spec)
            .auth(client.post("http://h:8080/v1/statement"))
            .build()
            .unwrap();
        assert!(plain.headers().get("X-Trino-Catalog").is_none());
        assert!(plain.headers().get("X-Trino-Schema").is_none());

        let with = TrinoConnector::new(&spec)
            .with_session("ice", "s")
            .auth(client.post("http://h:8080/v1/statement"))
            .build()
            .unwrap();
        assert_eq!(with.headers().get("X-Trino-Catalog").unwrap(), "ice");
        assert_eq!(with.headers().get("X-Trino-Schema").unwrap(), "s");

        let no_schema = TrinoConnector::new(&spec)
            .with_session("ice", "")
            .auth(client.post("http://h:8080/v1/statement"))
            .build()
            .unwrap();
        assert_eq!(no_schema.headers().get("X-Trino-Catalog").unwrap(), "ice");
        assert!(no_schema.headers().get("X-Trino-Schema").is_none());
    }

    // The basic-auth (impersonate) path must also carry X-Forwarded-Proto so the
    // coordinator does not reject the PASSWORD credential as insecure.
    #[test]
    fn basic_auth_path_sends_forwarded_proto() {
        let c = TrinoConnector::new(&ConnSpec {
            engine: Engine::Trino,
            host: "h".into(),
            port: 8080,
            options: ConnOptions {
                user: Some("egret_delegator".into()),
                impersonate: Some(true),
                ..Default::default()
            },
            secret: Some("delegator-pw".into()),
            session_user: Some("alice".into()),
            bearer_token: None,
        });
        let client = reqwest::Client::new();
        let req = c
            .auth(client.post("http://h:8080/v1/statement"))
            .build()
            .unwrap();
        let h = req.headers();
        assert_eq!(h.get("X-Forwarded-Proto").unwrap(), "https");
        assert_eq!(h.get("X-Trino-User").unwrap(), "alice");
        assert!(h
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("Basic "));
    }

    /// A pinned self-signed cert (jwt-mode/TLS) must build a working client.
    #[test]
    fn build_client_pins_tls_cert() {
        let params = rcgen::CertificateParams::new(vec!["coord.svc".to_string()]).unwrap();
        let key = rcgen::KeyPair::generate().unwrap();
        let pem = params.self_signed(&key).unwrap().pem();
        let c = TrinoConnector::new(&ConnSpec {
            engine: Engine::Trino,
            host: "coord.svc".into(),
            port: 8443,
            options: ConnOptions {
                http_scheme: Some("https".into()),
                tls_cert: Some(pem),
                ..Default::default()
            },
            secret: None,
            session_user: Some("alice".into()),
            bearer_token: Some("eyJ.jwt.sig".into()),
        });
        assert!(c.secure);
        assert!(c.client().is_ok());
    }

    /// **The pool's safety property, asserted where the projection lives.**
    ///
    /// Two connections to one coordinator that differ in EVERY identity a Trino
    /// request can carry — the basic-auth secret, the impersonated
    /// `X-Trino-User`, and the per-user OIDC bearer — must produce the same
    /// cache key, because the client they share carries none of those. The
    /// request each one builds must still carry its own.
    ///
    /// Mutating `client_key` to include any of the three makes the first
    /// assertion fail; moving an identity onto the client (a `default_headers`
    /// on the shared builder, say) is caught by
    /// `a_shared_client_never_carries_one_principals_cookie_to_another` and
    /// `two_principals_on_one_coordinator_share_a_client_and_never_an_identity`
    /// in `tests/connector_trino_pool.rs`, which drive a real server.
    #[test]
    fn the_client_key_ignores_every_identity_field() {
        let base = |secret: Option<&str>, user: &str, bearer: Option<&str>| ConnSpec {
            engine: Engine::Trino,
            host: "shared.svc".into(),
            port: 8080,
            options: ConnOptions {
                impersonate: Some(true),
                ..Default::default()
            },
            secret: secret.map(str::to_string),
            session_user: Some(user.to_string()),
            bearer_token: bearer.map(str::to_string),
        };
        let alice = TrinoConnector::new(&base(Some("secret-a"), "alice", Some("jwt-alice")));
        let bob = TrinoConnector::new(&base(Some("secret-b"), "bob", Some("jwt-bob")));

        assert_eq!(
            alice.client_key(),
            bob.client_key(),
            "two principals on one coordinator must share one client"
        );

        // And the shared key has not cost them their identities: each request
        // still carries its own.
        let client = reqwest::Client::new();
        let req_a = alice
            .auth(client.get("http://shared.svc:8080/v1/statement"))
            .build()
            .expect("build alice");
        let req_b = bob
            .auth(client.get("http://shared.svc:8080/v1/statement"))
            .build()
            .expect("build bob");
        assert_eq!(req_a.headers().get("X-Trino-User").unwrap(), "alice");
        assert_eq!(req_b.headers().get("X-Trino-User").unwrap(), "bob");
        assert_eq!(
            req_a.headers().get("authorization").unwrap(),
            "Bearer jwt-alice"
        );
        assert_eq!(
            req_b.headers().get("authorization").unwrap(),
            "Bearer jwt-bob"
        );
    }

    /// The cert is the one thing that MUST stay in the key: a client built with
    /// one coordinator's pinned root would trust the wrong CA for the other.
    #[test]
    fn the_client_key_separates_two_pinned_certs() {
        let spec = |pem: &str| ConnSpec {
            engine: Engine::Trino,
            host: "pinned.svc".into(),
            port: 8443,
            options: ConnOptions {
                http_scheme: Some("https".into()),
                tls_cert: Some(pem.to_string()),
                ..Default::default()
            },
            secret: None,
            session_user: None,
            bearer_token: None,
        };
        assert_ne!(
            TrinoConnector::new(&spec("-----PEM A-----")).client_key(),
            TrinoConnector::new(&spec("-----PEM B-----")).client_key()
        );
    }

    /// The deadline moved from the client to the request; a request that lost it
    /// would be unbounded.
    #[test]
    fn every_request_carries_the_callers_deadline() {
        let c = TrinoConnector::new(&ConnSpec {
            engine: Engine::Trino,
            host: "deadline.svc".into(),
            port: 8080,
            options: ConnOptions::default(),
            secret: None,
            session_user: None,
            bearer_token: None,
        });
        let client = reqwest::Client::new();
        let req = TrinoConnector::timed(
            c.auth(client.get("http://deadline.svc:8080/v1/statement")),
            std::time::Duration::from_secs(7),
        )
        .build()
        .expect("build");
        assert_eq!(req.timeout(), Some(&std::time::Duration::from_secs(7)));
    }

    // --- Retry-After parsing (the 503 path; the loop itself is driven against a
    // real server in `tests/connector_trino_pool.rs`) ---

    #[test]
    fn retry_after_reads_the_delta_seconds_form() {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert(reqwest::header::RETRY_AFTER, "2".parse().unwrap());
        assert_eq!(
            parse_retry_after(&h),
            Some(std::time::Duration::from_secs(2))
        );
    }

    #[test]
    fn retry_after_ignores_the_http_date_form_and_anything_unparseable() {
        // Deliberate: a date needs a clock comparison against a server whose
        // clock egret does not trust. Treated as absent, which falls back to
        // egret's own (shorter) backoff.
        for v in ["Wed, 21 Oct 2015 07:28:00 GMT", "soon", "-3"] {
            let mut h = reqwest::header::HeaderMap::new();
            h.insert(reqwest::header::RETRY_AFTER, v.parse().unwrap());
            assert_eq!(parse_retry_after(&h), None, "should ignore {v:?}");
        }
        assert_eq!(
            parse_retry_after(&reqwest::header::HeaderMap::new()),
            None,
            "an absent header is absent"
        );
    }

    /// **The cap on `Retry-After`, pinned on the function that applies it.**
    ///
    /// A coordinator answering `Retry-After: 3600` is asking egret to hold a
    /// caller's request for an hour; `describe`'s path would have obeyed, and a
    /// fixture that sends `Retry-After: 1` cannot tell the difference because
    /// one second IS the cap (`d.min(cap) == d` either way). This asks for an
    /// hour and asserts on the second.
    ///
    /// Mutation: delete `.min(TRINO_RETRY_MAX_BACKOFF)` from `retry_wait`'s
    /// `Some` arm and this fails with 3600s.
    #[test]
    fn an_absurd_retry_after_is_capped_at_one_sleep() {
        assert_eq!(
            retry_wait(1, Some(std::time::Duration::from_secs(3600))),
            TRINO_RETRY_MAX_BACKOFF,
            "the coordinator must not be able to choose how long egret waits"
        );
        // Under the cap the coordinator's number is used as asked — the header
        // is honoured, not ignored.
        assert_eq!(
            retry_wait(1, Some(std::time::Duration::from_millis(250))),
            std::time::Duration::from_millis(250)
        );
    }

    /// egret's own backoff doubles and is capped by the same ceiling.
    #[test]
    fn the_exponential_backoff_doubles_and_is_capped() {
        let ms = |n| std::time::Duration::from_millis(n);
        assert_eq!(retry_wait(1, None), ms(50));
        assert_eq!(retry_wait(2, None), ms(100));
        assert_eq!(retry_wait(3, None), ms(200));
        assert_eq!(retry_wait(4, None), ms(400));
        // The fifth attempt never happens (TRINO_RETRY_MAX_ATTEMPTS is 5, so
        // there are four retries) but the cap holds past it either way.
        assert_eq!(retry_wait(9, None), TRINO_RETRY_MAX_BACKOFF);
    }

    /// The bound the retry loop promises, summed over the ACTUAL waits the loop
    /// would take rather than over the constants.
    ///
    /// The worst case is every attempt carrying an absurd `Retry-After`; this
    /// sums exactly what `send` would sleep, so removing either cap moves this
    /// number.
    #[test]
    fn the_retry_budget_is_bounded() {
        let hour = Some(std::time::Duration::from_secs(3600));
        let worst: std::time::Duration = (1..TRINO_RETRY_MAX_ATTEMPTS)
            .map(|attempt| retry_wait(attempt, hour))
            .sum();
        assert!(
            worst <= TRINO_RETRY_MAX_TOTAL_WAIT,
            "retries may sleep at most {TRINO_RETRY_MAX_TOTAL_WAIT:?}, got {worst:?}"
        );
        let own: std::time::Duration = (1..TRINO_RETRY_MAX_ATTEMPTS)
            .map(|attempt| retry_wait(attempt, None))
            .sum();
        assert_eq!(
            own,
            std::time::Duration::from_millis(750),
            "50 + 100 + 200 + 400"
        );
    }

    /// What a 503 on a statement POST may re-send, and what it may not.
    ///
    /// The `INSERT` rows are the finding: `job.rs`'s `run_query` sends SQL Lab
    /// and notebook SQL straight through this connector, and `apply_sql_limit`
    /// leaves non-`SELECT` SQL unchanged, so a write really does travel this
    /// path.
    #[test]
    fn only_a_provable_read_may_be_re_posted() {
        for sql in [
            "SELECT 1",
            "select 1",
            "  \n SELECT * FROM t WHERE a = 1;",
            "WITH a AS (SELECT 1) SELECT * FROM a",
            "SHOW CATALOGS",
            "DESCRIBE t",
            "EXPLAIN SELECT 1",
            // The same, with the operand cased and spaced differently, so the
            // `EXPLAIN` arm is exercised as a rule and not as one string.
            "explain    with a as (select 1) select * from a",
            "(SELECT 1) UNION (SELECT 2)",
        ] {
            assert!(is_read_statement(sql), "{sql:?} is a read");
        }
        for sql in [
            "INSERT INTO t VALUES (1)",
            "insert into t select * from u",
            "CREATE TABLE t AS SELECT 1",
            "DELETE FROM t",
            "UPDATE t SET a = 1",
            "MERGE INTO t USING u ON t.a = u.a WHEN MATCHED THEN DELETE",
            "CALL system.runtime.kill_query('x')",
            "ALTER TABLE t ADD COLUMN c bigint",
            // **The finding.** `EXPLAIN ANALYZE <stmt>` RUNS `<stmt>`; it is not
            // a planner verb. A leading `explain` used to be proof enough, so
            // this classified as a read and an ingress-synthesised 503 would
            // have re-POSTed the INSERT up to four more times.
            "EXPLAIN ANALYZE INSERT INTO t SELECT * FROM u",
            "explain analyze insert into t select * from u",
            "EXPLAIN ANALYZE DELETE FROM t",
            // Executing too, and refused for the same reason even though
            // re-running it would merely be wasteful: the rule is that the
            // operand must PROVE itself, not that the harm is weighed per case.
            "EXPLAIN ANALYZE SELECT 1",
            // A plain EXPLAIN of a write plans rather than executes, so this one
            // is safe in fact and refused anyway — an unproven statement may
            // lose a retry, which is the affordable direction.
            "EXPLAIN INSERT INTO t VALUES (1)",
            "EXPLAIN (TYPE LOGICAL) SELECT 1",
            // Nothing wraps an EXPLAIN, so the recursion bound refuses it.
            "EXPLAIN EXPLAIN SELECT 1",
            // `EXPLAIN` with nothing to explain proves nothing.
            "EXPLAIN",
            // Read-shaped keywords that execute or write, kept off the
            // allow-list on purpose — see the doc comment.
            "EXECUTE my_prepared_insert",
            "ANALYZE hive.sales.orders",
            "PREPARE p FROM SELECT 1",
            // Multi-statement: the first keyword reads like a query and the
            // second one is not.
            "SELECT 1; INSERT INTO t VALUES (1)",
            // A leading comment is not stripped — conservative on purpose.
            "-- a note\nSELECT 1",
            "",
        ] {
            assert!(
                !is_read_statement(sql),
                "{sql:?} must not be re-POSTed after a 503"
            );
        }
    }

    /// Over genuine TLS the redundant `X-Forwarded-Proto` header is omitted.
    #[test]
    fn https_scheme_omits_forwarded_proto() {
        let c = TrinoConnector::new(&ConnSpec {
            engine: Engine::Trino,
            host: "coord.svc".into(),
            port: 8443,
            options: ConnOptions {
                http_scheme: Some("https".into()),
                ..Default::default()
            },
            secret: None,
            session_user: Some("alice".into()),
            bearer_token: Some("eyJ.jwt.sig".into()),
        });
        let client = reqwest::Client::new();
        let req = c
            .auth(client.post("https://coord.svc:8443/v1/statement"))
            .build()
            .unwrap();
        let h = req.headers();
        assert_eq!(h.get("X-Trino-User").unwrap(), "alice");
        assert_eq!(h.get("authorization").unwrap(), "Bearer eyJ.jwt.sig");
        assert!(h.get("X-Forwarded-Proto").is_none());
    }

    #[test]
    fn rebase_follows_nexturi_on_our_base_scheme_and_host() {
        let c = TrinoConnector::new(&ConnSpec {
            engine: Engine::Trino,
            host: "coord.svc".into(),
            port: 8080,
            options: ConnOptions::default(),
            secret: None,
            session_user: None,
            bearer_token: None,
        });
        // Trino echoes https (from X-Forwarded-Proto) + a different host; keep only
        // the path+query and use our own base (http://coord.svc:8080).
        assert_eq!(
            c.rebase("https://elsewhere:443/v1/statement/queued/abc/token/1?x=y"),
            "http://coord.svc:8080/v1/statement/queued/abc/token/1?x=y"
        );
        // A relative next path is just appended.
        assert_eq!(
            c.rebase("/v1/statement/executing/def/2"),
            "http://coord.svc:8080/v1/statement/executing/def/2"
        );
    }
}
