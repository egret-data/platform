//! Registry store errors.

use crate::config::ValidationError;

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error(transparent)]
    Invalid(#[from] ValidationError),
    #[error(transparent)]
    Secrets(#[from] egret_secrets::SecretsError),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}
