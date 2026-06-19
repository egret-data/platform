//! BI error + HTTP mapping.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

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

pub enum BiApiError {
    Auth(egret_core::Error),
    Domain(BiError),
}
impl From<egret_core::Error> for BiApiError {
    fn from(e: egret_core::Error) -> Self {
        BiApiError::Auth(e)
    }
}
impl From<BiError> for BiApiError {
    fn from(e: BiError) -> Self {
        BiApiError::Domain(e)
    }
}

impl IntoResponse for BiApiError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            BiApiError::Auth(e) => match e {
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
            BiApiError::Domain(e) => match &e {
                BiError::NotFound(_) => (StatusCode::NOT_FOUND, e.to_string()),
                BiError::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
                BiError::BadRequest(_) => (StatusCode::BAD_REQUEST, e.to_string()),
                BiError::Conflict(_) => (StatusCode::CONFLICT, e.to_string()),
                BiError::Engine(_) => (StatusCode::BAD_GATEWAY, e.to_string()),
                BiError::Db(_) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal error".to_string(),
                ),
            },
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}
