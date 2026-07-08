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

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = self.status_code();
        // Public-safe message: expose the specific message for client-facing
        // errors; hide internals for 5xx.
        let msg = match &self {
            Error::Internal(_) => "internal error".to_string(),
            Error::Unauthorized => "unauthorized".to_string(),
            Error::Forbidden => "forbidden".to_string(),
            Error::TooManyRequests => "too many requests".to_string(),
            other => other.to_string(),
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

/// The uniform API-error envelope every seam returns from its handlers.
///
/// `Auth` covers authn/authz/validation via the shared core mapping; `Domain`
/// is the seam's own error, which supplies its own `IntoResponse`. A handler
/// returns `Result<T, ApiError<MyDomainError>>` and `?` works on both a
/// `core::Error` and `MyDomainError` (given the seam adds `From<MyDomainError>`).
pub enum ApiError<D> {
    Auth(Error),
    Domain(D),
}

impl<D> From<Error> for ApiError<D> {
    fn from(e: Error) -> Self {
        ApiError::Auth(e)
    }
}

impl<D: IntoResponse> IntoResponse for ApiError<D> {
    fn into_response(self) -> Response {
        match self {
            ApiError::Auth(e) => e.into_response(),
            ApiError::Domain(d) => d.into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_codes_are_stable() {
        assert_eq!(
            Error::NotFound("x".into()).status_code(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            Error::InvalidInput("x".into()).status_code(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(Error::Unauthorized.status_code(), StatusCode::UNAUTHORIZED);
        assert_eq!(Error::Forbidden.status_code(), StatusCode::FORBIDDEN);
        assert_eq!(
            Error::Conflict("x".into()).status_code(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            Error::TooManyRequests.status_code(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            Error::Internal(anyhow::anyhow!("boom")).status_code(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn error_into_response_carries_status() {
        let resp = Error::Forbidden.into_response();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        let resp = Error::NotFound("user".into()).into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn api_error_auth_arm_uses_core_mapping() {
        let e: ApiError<StatusCode> = Error::Forbidden.into();
        assert_eq!(e.into_response().status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn api_error_domain_arm_delegates_to_d() {
        let e: ApiError<StatusCode> = ApiError::Domain(StatusCode::IM_A_TEAPOT);
        assert_eq!(e.into_response().status(), StatusCode::IM_A_TEAPOT);
    }
}
