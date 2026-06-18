//! Engine error + HTTP mapping.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("forbidden")]
    Forbidden,
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("connect: {0}")]
    Connect(String),
    #[error("engine: {0}")]
    Engine(String),
    #[error("timeout")]
    Timeout,
    #[error("cancelled")]
    Cancelled,
    #[error("limit: {0}")]
    Limit(String),
    #[error(transparent)]
    Storage(#[from] egret_storage::StorageError),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

impl From<egret_database::DatabaseError> for EngineError {
    fn from(e: egret_database::DatabaseError) -> Self {
        match e {
            egret_database::DatabaseError::NotFound(s) => EngineError::NotFound(s),
            other => EngineError::Engine(other.to_string()),
        }
    }
}

/// HTTP-facing error: an authz/validation failure or a domain error.
pub enum EngineApiError {
    Auth(egret_core::Error),
    Domain(EngineError),
}

impl From<egret_core::Error> for EngineApiError {
    fn from(e: egret_core::Error) -> Self {
        EngineApiError::Auth(e)
    }
}
impl From<EngineError> for EngineApiError {
    fn from(e: EngineError) -> Self {
        EngineApiError::Domain(e)
    }
}

impl IntoResponse for EngineApiError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            EngineApiError::Auth(e) => match e {
                egret_core::Error::NotFound(_) => (StatusCode::NOT_FOUND, "not found".to_string()),
                egret_core::Error::InvalidInput(m) => (StatusCode::BAD_REQUEST, m),
                egret_core::Error::Unauthorized => {
                    (StatusCode::UNAUTHORIZED, "unauthorized".to_string())
                }
                egret_core::Error::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
                egret_core::Error::Conflict(m) => (StatusCode::CONFLICT, m),
                egret_core::Error::TooManyRequests => (
                    StatusCode::TOO_MANY_REQUESTS,
                    "too many requests".to_string(),
                ),
                egret_core::Error::Internal(_) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal error".to_string(),
                ),
            },
            EngineApiError::Domain(e) => match &e {
                EngineError::NotFound(_) => (StatusCode::NOT_FOUND, e.to_string()),
                EngineError::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
                EngineError::BadRequest(_) | EngineError::Limit(_) => {
                    (StatusCode::BAD_REQUEST, e.to_string())
                }
                EngineError::Storage(_) | EngineError::Db(_) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal error".to_string(),
                ),
                _ => (StatusCode::BAD_REQUEST, e.to_string()),
            },
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}
