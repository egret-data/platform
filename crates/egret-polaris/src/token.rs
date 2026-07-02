//! Admin bearer-token acquisition via Keycloak client-credentials.
//!
//! Polaris's own `/v1/oauth/tokens` endpoint is deprecated-for-removal; in
//! MIXED auth mode Polaris accepts Keycloak OIDC bearer tokens. [`TokenSource`]
//! fetches a client-credentials token from Keycloak and caches it, refreshing
//! shortly before expiry.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::error::PolarisError;
use crate::transport::HttpTransport;

/// Refresh a cached token at least this long before its expiry.
const REFRESH_SKEW: Duration = Duration::from_secs(30);

/// Static configuration for talking to Polaris + its Keycloak issuer.
#[derive(Debug, Clone)]
pub struct PolarisConfig {
    /// Base URL of the Polaris management API (e.g. `http://polaris:8181`).
    pub base_url: String,
    /// Keycloak token endpoint (`.../protocol/openid-connect/token`).
    pub kc_token_url: String,
    /// Keycloak client id used for client-credentials.
    pub kc_client_id: String,
    /// Keycloak client secret.
    pub kc_client_secret: String,
    /// Optional OAuth scope to request.
    pub kc_scope: Option<String>,
}

/// Keycloak token endpoint response.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[allow(dead_code)]
    token_type: String,
    expires_in: u64,
}

/// Fetches and caches a Keycloak client-credentials bearer token.
///
/// Time is injected via the `now` argument to [`TokenSource::bearer`] so tests
/// are deterministic; the source never calls `Instant::now` itself.
pub struct TokenSource {
    token_url: String,
    client_id: String,
    client_secret: String,
    scope: Option<String>,
    /// Cached `(access_token, expires_at)`.
    cached: Mutex<Option<(String, Instant)>>,
}

impl TokenSource {
    /// Build a token source for a Keycloak client-credentials client.
    pub fn new(
        token_url: impl Into<String>,
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        scope: Option<String>,
    ) -> Self {
        Self {
            token_url: token_url.into(),
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            scope,
            cached: Mutex::new(None),
        }
    }

    /// Return a valid bearer token, refreshing it if there is no cached token
    /// or the cached one expires within [`REFRESH_SKEW`] of `now`.
    pub async fn bearer(
        &self,
        t: &dyn HttpTransport,
        now: Instant,
    ) -> Result<String, PolarisError> {
        // Fast path: reuse a still-fresh cached token.
        if let Some((tok, exp)) = self.cached.lock().unwrap().as_ref() {
            if *exp > now && *exp - now > REFRESH_SKEW {
                return Ok(tok.clone());
            }
        }

        // Refresh via client_credentials (form-encoded per OAuth spec).
        let mut form: Vec<(&str, &str)> = vec![
            ("grant_type", "client_credentials"),
            ("client_id", &self.client_id),
            ("client_secret", &self.client_secret),
        ];
        if let Some(scope) = &self.scope {
            form.push(("scope", scope));
        }

        let (status, body) = t.send_form("POST", &self.token_url, &form).await?;
        if status != 200 {
            let msg = String::from_utf8_lossy(&body).into_owned();
            return Err(PolarisError::Status(status, msg));
        }

        let parsed: TokenResponse = serde_json::from_slice(&body)?;
        let expires_at = now + Duration::from_secs(parsed.expires_in);
        *self.cached.lock().unwrap() = Some((parsed.access_token.clone(), expires_at));
        Ok(parsed.access_token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::FakeTransport;

    #[tokio::test]
    async fn token_source_fetches_and_caches() {
        let t = FakeTransport::new(vec![(
            200,
            br#"{"access_token":"AT","token_type":"bearer","expires_in":3600}"#.to_vec(),
        )]);
        let ts = TokenSource::new("http://kc/token", "cid", "sec", None);
        let now = Instant::now();
        assert_eq!(ts.bearer(&t, now).await.unwrap(), "AT");
        // Second call within validity window is served from cache.
        assert_eq!(ts.bearer(&t, now).await.unwrap(), "AT");
        assert_eq!(t.calls(), 1);

        // The single call was a form POST carrying the client-credentials grant.
        let (method, url, body) = t.last_request();
        assert_eq!(method, "POST");
        assert_eq!(url, "http://kc/token");
        assert_eq!(body["grant_type"], "client_credentials");
        assert_eq!(body["client_id"], "cid");
        assert_eq!(body["client_secret"], "sec");
    }

    #[tokio::test]
    async fn token_source_refetches_near_expiry() {
        let t = FakeTransport::new(vec![
            (
                200,
                br#"{"access_token":"AT1","token_type":"bearer","expires_in":60}"#.to_vec(),
            ),
            (
                200,
                br#"{"access_token":"AT2","token_type":"bearer","expires_in":60}"#.to_vec(),
            ),
        ]);
        let ts = TokenSource::new("http://kc/token", "cid", "sec", None);
        let now = Instant::now();
        assert_eq!(ts.bearer(&t, now).await.unwrap(), "AT1");
        assert_eq!(t.calls(), 1);

        // Advance well past the token's expiry (60s validity) so a refresh occurs.
        let later = now + Duration::from_secs(4000);
        assert_eq!(ts.bearer(&t, later).await.unwrap(), "AT2");
        assert_eq!(t.calls(), 2);
    }
}
