//! Error type for the Polaris management client.

/// Errors surfaced by the Polaris client and its HTTP transport.
#[derive(Debug, thiserror::Error)]
pub enum PolarisError {
    #[error("forbidden")]
    Forbidden,
    #[error("not found")]
    NotFound,
    #[error("conflict")]
    Conflict,
    #[error("polaris returned status {0}: {1}")]
    Status(u16, String),
    #[error("http error: {0}")]
    Http(String),
    #[error("decode error: {0}")]
    Decode(String),
}

impl From<reqwest::Error> for PolarisError {
    fn from(e: reqwest::Error) -> Self {
        PolarisError::Http(e.to_string())
    }
}

impl From<serde_json::Error> for PolarisError {
    fn from(e: serde_json::Error) -> Self {
        PolarisError::Decode(e.to_string())
    }
}
