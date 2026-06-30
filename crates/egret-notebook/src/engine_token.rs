//! The engine-token seam. Concrete minting (Keycloak refresh grant) is injected
//! from egret-bin, so this crate stays free of an HTTP/IdP dependency.
use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize)]
pub struct EngineToken {
    pub access_token: String,
    pub expires_at: DateTime<Utc>,
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

/// Test double: returns a canned token and records the last user.
#[derive(Default)]
pub struct FakeMinter {
    pub last: std::sync::Mutex<Option<Uuid>>,
}

#[async_trait::async_trait]
impl EngineTokenMinter for FakeMinter {
    async fn mint(&self, user_id: Uuid) -> Result<EngineToken, BrokerError> {
        *self.last.lock().unwrap() = Some(user_id);
        Ok(EngineToken {
            access_token: "fake.jwt.token".into(),
            expires_at: DateTime::<Utc>::from_timestamp(4102444800, 0).unwrap(), // 2100-01-01
        })
    }
}
