//! `perm` store errors.

#[derive(Debug, thiserror::Error)]
pub enum PermError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid data read back from the store: {0}")]
    Invalid(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}
