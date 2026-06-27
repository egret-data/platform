#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}
