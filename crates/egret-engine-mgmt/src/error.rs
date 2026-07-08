//! Engine-mgmt store errors.

#[derive(Debug, thiserror::Error)]
pub enum EngineMgmtError {
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}
