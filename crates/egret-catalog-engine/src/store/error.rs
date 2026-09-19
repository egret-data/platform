#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("in use")]
    InUse,
    #[error("bad request: {0}")]
    BadRequest(String),
    /// A state conflict that isn't a duplicate-name clash covered by
    /// `AlreadyExists` (e.g. a warehouse delete refused because its registry
    /// catalog is still attached to an engine).
    #[error("conflict: {0}")]
    Conflict(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}
