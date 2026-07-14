//! Engine-mgmt store errors.

#[derive(Debug, thiserror::Error)]
pub enum EngineMgmtError {
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("not found: {0}")]
    NotFound(String),
    /// The engine cannot be assigned to a workspace — e.g. it has not been
    /// configured for per-user JWT delegation, so no read-only connection can be
    /// materialized for it.
    #[error("engine is not assignable: {0}")]
    NotAssignable(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}
