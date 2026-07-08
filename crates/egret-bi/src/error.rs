//! BI domain error + its HTTP mapping. The auth/validation arm now lives in
//! `egret_core::ApiError` (shared); this file keeps only the BI-domain arm.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use egret_core::ApiError;

#[derive(Debug, thiserror::Error)]
pub enum BiError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("forbidden")]
    Forbidden,
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("engine: {0}")]
    Engine(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

impl From<egret_database::DatabaseError> for BiError {
    fn from(e: egret_database::DatabaseError) -> Self {
        match e {
            egret_database::DatabaseError::NotFound(s) => BiError::NotFound(s),
            other => BiError::Engine(other.to_string()),
        }
    }
}
impl From<egret_engine::EngineError> for BiError {
    fn from(e: egret_engine::EngineError) -> Self {
        match e {
            egret_engine::EngineError::NotFound(s) => BiError::NotFound(s),
            other => BiError::Engine(other.to_string()),
        }
    }
}

/// Domain arm only — the shared `Auth` arm is in `egret_core::ApiError`.
impl IntoResponse for BiError {
    fn into_response(self) -> Response {
        let (status, msg) = match &self {
            BiError::NotFound(_) => (StatusCode::NOT_FOUND, self.to_string()),
            BiError::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
            BiError::BadRequest(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            BiError::Conflict(_) => (StatusCode::CONFLICT, self.to_string()),
            BiError::Engine(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
            BiError::Db(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            ),
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

/// The BI handler error type. `From<egret_core::Error>` comes from core;
/// this adds the domain-side `From` so `?` works on `BiError` too.
pub type BiApiError = ApiError<BiError>;

impl From<BiError> for BiApiError {
    fn from(e: BiError) -> Self {
        ApiError::Domain(e)
    }
}
