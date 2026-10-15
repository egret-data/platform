//! egret mini machine-IdP error type.

#[derive(Debug, thiserror::Error)]
pub enum IdpError {
    /// client_id/secret did not verify.
    #[error("invalid client")]
    InvalidClient,
    /// A required form field is missing or malformed (RFC 6749 §5.2
    /// `invalid_request`). The `&'static str` names the problem for the
    /// `error_description`; it never echoes request input.
    #[error("invalid request: {0}")]
    InvalidRequest(&'static str),
    /// The presented grant (a token-exchange `subject_token`) is invalid,
    /// expired, foreign, or no longer backed by a client that may mint
    /// (RFC 6749 §5.2 `invalid_grant`).
    #[error("invalid grant")]
    InvalidGrant,
    /// grant_type other than `client_credentials` or token exchange.
    #[error("unsupported grant type")]
    UnsupportedGrant,
    /// Too many token requests from one `ip:client_id` pair.
    #[error("too many token requests")]
    RateLimited,
    /// No signing key could be produced.
    #[error("no signing key")]
    NoKey,
    /// RSA/JWT crypto failure.
    #[error("crypto error: {0}")]
    Crypto(String),
    /// Persistence failure.
    #[error("db error: {0}")]
    Db(String),
    /// Every verifier that recognised the request errored out before any could
    /// confirm or deny the client — NOT the same as `InvalidClient`, which means
    /// every verifier positively looked and found nothing. Returning
    /// `InvalidClient` here would be a lie: validity was never actually checked.
    #[error("client verification unavailable: {0}")]
    VerificationUnavailable(String),
}

impl From<sqlx::Error> for IdpError {
    fn from(e: sqlx::Error) -> Self {
        IdpError::Db(e.to_string())
    }
}
