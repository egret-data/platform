//! The engine-token seam. Concrete minting (the machine-IdP signer) is injected
//! from egret-bin, so this crate stays free of an HTTP/IdP dependency. Moved
//! verbatim from `egret-notebook` (now deleted, Data Apps P6); every consumer
//! imports these types from here directly.
use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Clone, serde::Serialize, utoipa::ToSchema)]
pub struct EngineToken {
    pub access_token: String,
    pub expires_at: DateTime<Utc>,
}

// Redact the bearer token from Debug output (consistent with `OidcLogin`).
impl std::fmt::Debug for EngineToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineToken")
            .field("access_token", &"[redacted]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BrokerError {
    #[error("no stored refresh token for user")]
    NoRefreshToken,
    #[error("idp refresh failed: {0}")]
    Idp(String),
    #[error(transparent)]
    Other(#[from] egret_core::Error),
}

#[async_trait::async_trait]
pub trait EngineTokenMinter: Send + Sync {
    /// Mint a fresh, short-lived engine access token for `user_id`.
    async fn mint(&self, user_id: Uuid) -> Result<EngineToken, BrokerError>;
}
