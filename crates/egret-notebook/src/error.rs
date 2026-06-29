//! Notebook HTTP error mapping — thin wrapper over `egret_core::Error`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use egret_core::Error;

#[derive(Debug)]
pub struct NotebookApiError(pub Error);

impl From<Error> for NotebookApiError {
    fn from(e: Error) -> Self {
        Self(e)
    }
}

impl From<sqlx::Error> for NotebookApiError {
    fn from(e: sqlx::Error) -> Self {
        Self(Error::Internal(anyhow::anyhow!(e.to_string())))
    }
}

impl IntoResponse for NotebookApiError {
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
