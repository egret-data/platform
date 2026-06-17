//! Shared error and result types for all Egret crates.

use thiserror::Error;

/// The canonical error type returned across crate boundaries.
#[derive(Debug, Error)]
pub enum Error {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("unauthorized")]
    Unauthorized,

    #[error("forbidden")]
    Forbidden,

    #[error("conflict: {0}")]
    Conflict(String),

    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

/// Convenience alias used throughout the codebase.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_messages_are_stable() {
        assert_eq!(Error::NotFound("user".into()).to_string(), "not found: user");
        assert_eq!(Error::Unauthorized.to_string(), "unauthorized");
        assert_eq!(Error::Forbidden.to_string(), "forbidden");
    }

    #[test]
    fn wraps_anyhow_via_from() {
        let e: Error = anyhow::anyhow!("boom").into();
        assert!(matches!(e, Error::Internal(_)));
    }
}
