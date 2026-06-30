//! Keycloak-backed `EngineTokenMinter`: refresh-token grant → short-lived
//! engine access token; rotates the stored refresh token when the IdP returns one.
use async_trait::async_trait;
use chrono::{Duration, Utc};
use egret_auth::oidc_refresh::OidcRefreshRepo;
use egret_notebook::engine_token::{BrokerError, EngineToken, EngineTokenMinter};
use egret_secrets::SecretStore;
use uuid::Uuid;

/// The IdP refresh-grant call (seam so the broker is unit-testable without HTTP).
#[async_trait]
pub trait OidcTokenRefresher: Send + Sync {
    async fn refresh(&self, refresh_token: &str) -> Result<RefreshedToken, String>;
}

pub struct RefreshedToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in_secs: i64,
}

/// Real impl: POST the Keycloak token endpoint with grant_type=refresh_token.
pub struct KeycloakRefresher {
    pub http: reqwest::Client,
    pub token_url: String,
    pub client_id: String,
    pub client_secret: String,
}

#[async_trait]
impl OidcTokenRefresher for KeycloakRefresher {
    async fn refresh(&self, refresh_token: &str) -> Result<RefreshedToken, String> {
        #[derive(serde::Deserialize)]
        struct Resp {
            access_token: String,
            refresh_token: Option<String>,
            expires_in: i64,
        }
        let resp = self
            .http
            .post(&self.token_url)
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", &self.client_id),
                ("client_secret", &self.client_secret),
                ("refresh_token", refresh_token),
                ("scope", "openid offline_access"),
            ])
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("token endpoint {}", resp.status()));
        }
        let r: Resp = resp.json().await.map_err(|e| e.to_string())?;
        Ok(RefreshedToken {
            access_token: r.access_token,
            refresh_token: r.refresh_token,
            expires_in_secs: r.expires_in,
        })
    }
}

pub struct EngineTokenBroker<R: OidcTokenRefresher> {
    secrets: SecretStore,
    repo: OidcRefreshRepo,
    refresher: R,
}

impl<R: OidcTokenRefresher> EngineTokenBroker<R> {
    pub fn new(secrets: SecretStore, repo: OidcRefreshRepo, refresher: R) -> Self {
        Self {
            secrets,
            repo,
            refresher,
        }
    }
}

#[async_trait]
impl<R: OidcTokenRefresher> EngineTokenMinter for EngineTokenBroker<R> {
    async fn mint(&self, user_id: Uuid) -> Result<EngineToken, BrokerError> {
        let secret_id = self
            .repo
            .get(user_id)
            .await?
            .ok_or(BrokerError::NoRefreshToken)?;
        let refresh_token = self
            .secrets
            .get(secret_id)
            .await
            .map_err(|e| BrokerError::Idp(e.to_string()))?;
        let refreshed = self
            .refresher
            .refresh(&refresh_token)
            .await
            .map_err(BrokerError::Idp)?;
        if let Some(new_rt) = refreshed.refresh_token {
            if new_rt != refresh_token {
                self.secrets
                    .update(secret_id, &new_rt)
                    .await
                    .map_err(|e| BrokerError::Idp(e.to_string()))?;
            }
        }
        Ok(EngineToken {
            access_token: refreshed.access_token,
            expires_at: Utc::now() + Duration::seconds(refreshed.expires_in_secs),
        })
    }
}
