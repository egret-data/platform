//! Error type mapping AuthzError → HTTP status for the Ranger REST surface.

use crate::AuthzError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

pub struct RangerError(pub AuthzError);

impl From<AuthzError> for RangerError {
    fn from(e: AuthzError) -> Self {
        Self(e)
    }
}

impl IntoResponse for RangerError {
    fn into_response(self) -> Response {
        let (status, msg) = match &self.0 {
            AuthzError::NotFound(m) => (StatusCode::NOT_FOUND, m.clone()),
            AuthzError::AlreadyExists(m) => (StatusCode::CONFLICT, m.clone()),
            AuthzError::BadRequest(m) => (StatusCode::BAD_REQUEST, m.clone()),
            AuthzError::Json(e) => (StatusCode::BAD_REQUEST, e.to_string()),
            AuthzError::Db(e) => {
                tracing::error!(error = %e, "ranger: database error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal server error".to_string(),
                )
            }
        };
        (
            status,
            Json(serde_json::json!({ "msg": msg, "statusCode": status.as_u16() })),
        )
            .into_response()
    }
}
