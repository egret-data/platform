//! Error type mapping AuthzError → HTTP status for the Ranger REST surface.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use egret_authz::AuthzError;

pub struct RangerError(pub AuthzError);

impl From<AuthzError> for RangerError {
    fn from(e: AuthzError) -> Self {
        Self(e)
    }
}

impl IntoResponse for RangerError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            AuthzError::NotFound(_) => StatusCode::NOT_FOUND,
            AuthzError::AlreadyExists(_) => StatusCode::CONFLICT,
            AuthzError::BadRequest(_) | AuthzError::Json(_) => StatusCode::BAD_REQUEST,
            AuthzError::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (
            status,
            Json(serde_json::json!({ "msg": self.0.to_string(), "statusCode": status.as_u16() })),
        )
            .into_response()
    }
}
