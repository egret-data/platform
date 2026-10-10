#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("object not found: {0}")]
    NotFound(String),
    #[error("s3 error: {0}")]
    S3(String),
    #[error("signing error: {0}")]
    Signing(String),
    #[error("this object store does not implement {0}")]
    Unsupported(&'static str),
}
