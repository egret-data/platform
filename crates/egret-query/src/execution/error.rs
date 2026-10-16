//! Engine error + HTTP mapping.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use egret_core::ApiError;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("forbidden")]
    Forbidden,
    /// The engine answered 401: it could not authenticate the identity egret
    /// presented (bad/expired JWT, an empty or unreachable JWKS). Distinct from
    /// [`EngineError::Forbidden`], which is an authenticated caller being denied.
    /// The payload is the engine's own reason, logged and never sent to a client.
    #[error("engine authentication failed")]
    AuthFailed(String),
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

impl From<crate::registry::DatabaseError> for EngineError {
    fn from(e: crate::registry::DatabaseError) -> Self {
        match e {
            crate::registry::DatabaseError::NotFound(s) => EngineError::NotFound(s),
            other => EngineError::Engine(other.to_string()),
        }
    }
}

/// Domain arm only — the shared `Auth` arm is in `egret_core::ApiError`.
impl IntoResponse for EngineError {
    fn into_response(self) -> Response {
        let (status, msg) = match &self {
            EngineError::NotFound(_) => (StatusCode::NOT_FOUND, self.to_string()),
            EngineError::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
            EngineError::AuthFailed(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
            EngineError::BadRequest(_) | EngineError::Limit(_) => {
                (StatusCode::BAD_REQUEST, self.to_string())
            }
            EngineError::Storage(_) | EngineError::Db(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            ),
            _ => (StatusCode::BAD_REQUEST, self.to_string()),
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

pub type EngineApiError = ApiError<EngineError>;

impl From<EngineError> for EngineApiError {
    fn from(e: EngineError) -> Self {
        ApiError::Domain(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_401_and_403_read_differently_on_the_wire() {
        let auth = EngineError::AuthFailed("jwks empty".into()).into_response();
        let denied = EngineError::Forbidden.into_response();
        assert_eq!(auth.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            EngineError::AuthFailed("secret detail".into()).to_string(),
            "engine authentication failed",
            "the engine's reason is for the log, not the message"
        );
    }
}
