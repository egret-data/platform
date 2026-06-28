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
    /// A named secret with this name already exists in the workspace.
    #[error("secret named {0:?} already exists")]
    AlreadyExists(String),
    /// The named secret is still referenced by connections and cannot be deleted.
    #[error("secret is in use by {0} connection(s)")]
    InUse(i64),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}
