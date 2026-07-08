//! HTTP mapping for the canonical `egret_core::Error` (feature = "http").
//!
//! This is the single source of truth for how core errors become HTTP
//! responses. Previously this match arm was copy-pasted in 7 crates.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::Error;

impl Error {
    /// The HTTP status this error maps to.
    pub fn status_code(&self) -> StatusCode {
        match self {
            Error::NotFound(_) => StatusCode::NOT_FOUND,
            Error::InvalidInput(_) => StatusCode::BAD_REQUEST,
            Error::Unauthorized => StatusCode::UNAUTHORIZED,
            Error::Forbidden => StatusCode::FORBIDDEN,
            Error::Conflict(_) => StatusCode::CONFLICT,
            Error::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
            Error::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_codes_are_stable() {
        assert_eq!(Error::NotFound("x".into()).status_code(), StatusCode::NOT_FOUND);
        assert_eq!(Error::InvalidInput("x".into()).status_code(), StatusCode::BAD_REQUEST);
        assert_eq!(Error::Unauthorized.status_code(), StatusCode::UNAUTHORIZED);
        assert_eq!(Error::Forbidden.status_code(), StatusCode::FORBIDDEN);
        assert_eq!(Error::Conflict("x".into()).status_code(), StatusCode::CONFLICT);
        assert_eq!(Error::TooManyRequests.status_code(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            Error::Internal(anyhow::anyhow!("boom")).status_code(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
