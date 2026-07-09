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
        }
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
    /// With a bearer JWT the identity comes from the token (Trino
    /// `principal-field`), so `X-Trino-User` is NOT sent — sending it would be
    /// treated as impersonation and denied (verified live in F2-P5). Without a
    /// bearer, send `X-Trino-User` (+ basic-auth when a secret is set).
    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(jwt) = &self.bearer {
            req.bearer_auth(jwt)
        } else {
            let req = req.header("X-Trino-User", self.x_trino_user());
            match &self.secret {
                Some(s) => req.basic_auth(self.auth_user(), Some(s)),
                None => req,
            }
        }
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
        let client = reqwest::Client::builder()
            .timeout(limits.timeout)
            .build()
            .map_err(|e| EngineError::Connect(e.to_string()))?;

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
            let next = resp.next_uri.take();
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
        return Err(EngineError::Engine(format!("http {}", resp.status())));
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
}
