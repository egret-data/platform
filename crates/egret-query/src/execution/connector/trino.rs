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
        }
    }

    /// Build the reqwest client, pinning the coordinator's self-signed cert as a
    /// trusted root when `tls_cert` is set (jwt-mode/TLS). Pinning supersedes
    /// `danger_accept_invalid_certs`; without a pinned cert the client falls back
    /// to system roots (unchanged for plain-HTTP engines).
    fn build_client(&self, timeout: std::time::Duration) -> Result<reqwest::Client, EngineError> {
        let mut b = reqwest::Client::builder().timeout(timeout);
        if let Some(pem) = &self.tls_cert {
            let cert = reqwest::Certificate::from_pem(pem.as_bytes())
                .map_err(|e| EngineError::Connect(format!("engine cert: {e}")))?;
            b = b.add_root_certificate(cert);
        }
        b.build().map_err(|e| EngineError::Connect(e.to_string()))
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
    async fn run_statement(
        &self,
        client: &reqwest::Client,
        req: reqwest::RequestBuilder,
        limits: &Limits,
        cancel: CancellationToken,
    ) -> Result<TabularResult, EngineError> {
        let cap = limits.max_rows;
        let mut columns: Vec<ColumnMeta> = Vec::new();
        let mut rows: Vec<Vec<serde_json::Value>> = Vec::new();
        let mut bytes_scanned: Option<u64> = None;

        let mut resp: TrinoResp = send(req).await?;
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
                    let _ = self.auth(client.delete(uri)).send().await;
                }
                return Err(EngineError::Cancelled);
            }
            if rows.len() > cap {
                if let Some(uri) = &next {
                    let _ = self.auth(client.delete(uri)).send().await;
                }
                break;
            }
            match next {
                None => break,
                Some(uri) => {
                    resp = send(self.auth(client.get(&uri))).await?;
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
        let client = self.build_client(limits.timeout)?;

        let session = format!("query_max_run_time={}s", limits.timeout.as_secs().max(1));
        let req = self.auth(
            client
                .post(format!("{}/v1/statement", self.base_url))
                .header("X-Trino-Session", session)
                .body(sql.to_string()),
        );

        self.run_statement(&client, req, limits, cancel).await
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
        let client = self.build_client(limits.timeout)?;
        let req = self.prepared_request(&client, sql, params, limits);

        self.run_statement(&client, req, limits, cancel).await
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
        self.auth(
            client
                .post(format!("{}/v1/statement", self.base_url))
                .header("X-Trino-Session", session)
                .header("X-Trino-Prepared-Statement", prepared_header)
                .body(body),
        )
    }
}

async fn send(req: reqwest::RequestBuilder) -> Result<TrinoResp, EngineError> {
    let resp = req
        .send()
        .await
        .map_err(|e| EngineError::Connect(e.to_string()))?;
    if !resp.status().is_success() {
        let status = resp.status();
        // Per-user delegation: a 401/403 means the engine rejected THIS user's
        // identity or denied the tables (bad/expired JWT, or a Ranger/file-ACL deny).
        // Surface a clean Forbidden (→ HTTP 403 with a data-access message) instead of
        // an opaque engine error, so the BI client can explain it.
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(EngineError::Forbidden);
        }
        return Err(EngineError::Engine(format!("http {status}")));
    }
    resp.json::<TrinoResp>()
        .await
        .map_err(|e| EngineError::Engine(format!("decode: {e}")))
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
        assert!(c.build_client(std::time::Duration::from_secs(5)).is_ok());
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
