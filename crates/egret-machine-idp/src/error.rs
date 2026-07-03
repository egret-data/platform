//! egret mini machine-IdP error type.

#[derive(Debug, thiserror::Error)]
pub enum IdpError {
    /// client_id/secret did not verify.
    #[error("invalid client")]
    InvalidClient,
    /// grant_type other than `client_credentials`.
    #[error("unsupported grant type")]
    UnsupportedGrant,
    /// No signing key could be produced.
    #[error("no signing key")]
    NoKey,
    /// RSA/JWT crypto failure.
    #[error("crypto error: {0}")]
    Crypto(String),
    /// Persistence failure.
    #[error("db error: {0}")]
    Db(String),
}

impl From<sqlx::Error> for IdpError {
    fn from(e: sqlx::Error) -> Self {
        IdpError::Db(e.to_string())
    }
}
