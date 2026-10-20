//! Error type mapping PolicyStoreError → HTTP status for the Ranger REST surface.

use crate::PolicyStoreError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

pub enum RangerError {
    Store(PolicyStoreError),
    /// The caller authenticated but may not read this resource (an engine
    /// credential asking for a profile its engine is not bound to).
    Forbidden,
}

impl RangerError {
    pub fn forbidden() -> Self {
        Self::Forbidden
    }
}

impl From<PolicyStoreError> for RangerError {
    fn from(e: PolicyStoreError) -> Self {
        Self::Store(e)
    }
}

impl IntoResponse for RangerError {
    fn into_response(self) -> Response {
        let store_err = match self {
            Self::Store(e) => e,
            Self::Forbidden => {
                let status = StatusCode::FORBIDDEN;
                return (
                    status,
                    Json(serde_json::json!({ "msg": "forbidden", "statusCode": status.as_u16() })),
                )
                    .into_response();
            }
        };
        let (status, msg) = match &store_err {
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
