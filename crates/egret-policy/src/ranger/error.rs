//! Error type mapping PolicyStoreError → HTTP status for the Ranger REST surface.

use crate::PolicyStoreError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

pub struct RangerError(pub PolicyStoreError);

impl From<PolicyStoreError> for RangerError {
    fn from(e: PolicyStoreError) -> Self {
        Self(e)
    }
}

impl IntoResponse for RangerError {
    fn into_response(self) -> Response {
        let (status, msg) = match &self.0 {
            PolicyStoreError::NotFound(m) => (StatusCode::NOT_FOUND, m.clone()),
            PolicyStoreError::AlreadyExists(m) => (StatusCode::CONFLICT, m.clone()),
            PolicyStoreError::BadRequest(m) => (StatusCode::BAD_REQUEST, m.clone()),
            // Unreachable in practice: the Ranger REST serving surface this
            // error type belongs to is a read-only download surface for
            // engine plugins (`enabled_policies_for_service` etc.), never a
            // caller of the Task 8-guarded write paths. Mapped the same way
            // as `AlreadyExists` for exhaustiveness, not because this path
            // can actually produce it.
            PolicyStoreError::CompilerOwned(m) | PolicyStoreError::InfrastructureOwned(m) => {
                (StatusCode::CONFLICT, m.clone())
            }
            PolicyStoreError::Json(e) => (StatusCode::BAD_REQUEST, e.to_string()),
            PolicyStoreError::Db(e) => {
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
