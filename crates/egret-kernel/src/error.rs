//! Kernel error types — mirrors `egret_notebook::error`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use egret_core::Error;

/// Domain-level kernel error.
#[derive(Debug, thiserror::Error)]
pub enum KernelError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("spawn failed: {0}")]
    Spawn(String),
    #[error("internal: {0}")]
    Internal(#[from] anyhow::Error),
    #[error("gateway error: {0}")]
    Gateway(String),
}

/// HTTP-layer wrapper that maps `KernelError` / `egret_core::Error` to responses.
#[derive(Debug)]
pub struct KernelApiError(pub Error);

impl From<Error> for KernelApiError {
    fn from(e: Error) -> Self {
        Self(e)
    }
}

impl From<sqlx::Error> for KernelApiError {
    fn from(e: sqlx::Error) -> Self {
        Self(Error::Internal(anyhow::anyhow!(e)))
    }
}

impl From<KernelError> for KernelApiError {
    fn from(e: KernelError) -> Self {
        Self(Error::Internal(anyhow::anyhow!(e)))
    }
}

impl IntoResponse for KernelApiError {
    fn into_response(self) -> Response {
        let (code, msg) = match &self.0 {
            Error::NotFound(m) => (StatusCode::NOT_FOUND, m.clone()),
            Error::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
            Error::InvalidInput(m) => (StatusCode::BAD_REQUEST, m.clone()),
            Error::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".to_string()),
            Error::Conflict(m) => (StatusCode::CONFLICT, m.clone()),
            Error::TooManyRequests => (
                StatusCode::TOO_MANY_REQUESTS,
                "too many requests".to_string(),
            ),
            Error::Internal(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            ),
        };
        (code, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}
