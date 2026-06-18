//! Domain + HTTP error types for the database registry.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use egret_secrets::SecretsError;

#[derive(Debug, thiserror::Error)]
pub enum DatabaseError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("secrets master_key is not configured")]
    SecretsNotConfigured,
    #[error(transparent)]
    Secrets(SecretsError),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

impl From<SecretsError> for DatabaseError {
    fn from(e: SecretsError) -> Self {
        match e {
            SecretsError::NotConfigured => DatabaseError::SecretsNotConfigured,
            SecretsError::Db(db) => DatabaseError::Db(db),
            other => DatabaseError::Secrets(other),
        }
    }
}

/// HTTP-facing error: either an authz failure (`egret_core::Error`) or a domain error.
pub enum DbApiError {
    Auth(egret_core::Error),
    Domain(DatabaseError),
}

impl From<egret_core::Error> for DbApiError {
    fn from(e: egret_core::Error) -> Self {
        DbApiError::Auth(e)
    }
}
impl From<DatabaseError> for DbApiError {
    fn from(e: DatabaseError) -> Self {
        DbApiError::Domain(e)
    }
}

impl IntoResponse for DbApiError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            DbApiError::Auth(e) => match e {
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
            DbApiError::Domain(e) => match &e {
                DatabaseError::NotFound(_) => (StatusCode::NOT_FOUND, e.to_string()),
                DatabaseError::AlreadyExists(_) => (StatusCode::CONFLICT, e.to_string()),
                DatabaseError::BadRequest(_) => (StatusCode::BAD_REQUEST, e.to_string()),
                DatabaseError::SecretsNotConfigured => (StatusCode::BAD_REQUEST, e.to_string()),
                DatabaseError::Secrets(_) | DatabaseError::Db(_) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal error".to_string(),
                ),
            },
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}
