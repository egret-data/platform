//! Errors for credential encryption and storage.

#[derive(Debug, thiserror::Error)]
pub enum SecretsError {
    /// Encryption/decryption/key failure (message is non-sensitive).
    #[error("crypto error: {0}")]
    Crypto(String),
    /// No master key configured but an encrypted operation was attempted.
    #[error("secrets master_key is not configured")]
    NotConfigured,
    /// Secret row not found.
    #[error("secret not found")]
    NotFound,
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}
