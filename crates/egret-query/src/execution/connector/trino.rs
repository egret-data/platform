//! Trino connector — REST statement protocol with row-cap, timeout, cancellation.

use async_trait::async_trait;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use egret_core::{ColumnMeta, TabularResult};

use super::{ConnSpec, Connector};
use crate::execution::error::EngineError;
use crate::execution::model::Limits;

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
        TrinoConnector {
            base_url: format!("{scheme}://{}:{}", spec.host, spec.port),
            auth_user,
            session_user,
            secret: spec.secret.clone(),
            bearer: spec.bearer_token.clone(),
            secure: scheme == "https",
            tls_cert: spec.options.tls_cert.clone(),
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

#[async_trait]
impl Connector for TrinoConnector {
    async fn execute(
        &self,
        sql: &str,
        limits: &Limits,
        cancel: CancellationToken,
    ) -> Result<TabularResult, EngineError> {
        let client = self.build_client(limits.timeout)?;

        let session = format!("query_max_run_time={}s", limits.timeout.as_secs().max(1));
        let req = self.auth(
            client
                .post(format!("{}/v1/statement", self.base_url))
                .header("X-Trino-Session", session)
                .body(sql.to_string()),
        );

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
