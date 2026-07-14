//! Domain + HTTP error types for the database registry.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use egret_core::ApiError;
use egret_secrets::SecretsError;

#[derive(Debug, thiserror::Error)]
pub enum DatabaseError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("conflict: {0}")]
    Conflict(String),
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

/// Domain arm only — the shared `Auth` arm is in `egret_core::ApiError`.
impl IntoResponse for DatabaseError {
    fn into_response(self) -> Response {
        let (status, msg) = match &self {
            DatabaseError::NotFound(_) => (StatusCode::NOT_FOUND, self.to_string()),
            DatabaseError::AlreadyExists(_) => (StatusCode::CONFLICT, self.to_string()),
            DatabaseError::Conflict(_) => (StatusCode::CONFLICT, self.to_string()),
            DatabaseError::BadRequest(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            DatabaseError::SecretsNotConfigured => (StatusCode::BAD_REQUEST, self.to_string()),
            DatabaseError::Secrets(_) | DatabaseError::Db(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            ),
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

pub type DbApiError = ApiError<DatabaseError>;

impl From<DatabaseError> for DbApiError {
    fn from(e: DatabaseError) -> Self {
        ApiError::Domain(e)
    }
}
