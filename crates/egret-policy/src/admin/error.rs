//! HTTP error for the admin Policies router. Mirrors the arms `egret-controlplane`'s
//! `CpError` produced for these routes (status codes + `{"error": msg}` body
//! unchanged) so the moved handlers behave identically after the relocation.
use crate::PolicyStoreError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

use super::reader::EngineReadError;

#[derive(Debug, thiserror::Error)]
pub enum PolicyHttpError {
    /// Authenticated but lacks the required `policy:*` permission.
    #[error("forbidden")]
    Forbidden,
    #[error("not found")]
    NotFound,
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("conflict: {0}")]
    Conflict(String),
    /// A mask/row-filter overlap (SP4d-4 Task 3 fix round 1, M1): 409 with the
    /// structured body every surface answers it with — `{error, code:
    /// "rule_overlap", rules, coordinate, kind}` — served as is.
    #[error("rule overlap")]
    Overlap { body: serde_json::Value },
    #[error("internal error")]
    Internal,
}

impl From<PolicyStoreError> for PolicyHttpError {
    fn from(e: PolicyStoreError) -> Self {
        match e {
            PolicyStoreError::NotFound(_) => PolicyHttpError::NotFound,
            PolicyStoreError::AlreadyExists(s) => PolicyHttpError::AlreadyExists(s),
            PolicyStoreError::BadRequest(s) => PolicyHttpError::BadRequest(s),
            PolicyStoreError::CompilerOwned(s) | PolicyStoreError::InfrastructureOwned(s) => {
                PolicyHttpError::Conflict(s)
            }
            PolicyStoreError::Db(_) | PolicyStoreError::Json(_) => PolicyHttpError::Internal,
        }
    }
}

/// `egret_core::Error` is what `require_permission` (egret-auth) returns.
impl From<egret_core::Error> for PolicyHttpError {
    fn from(e: egret_core::Error) -> Self {
        match e {
            egret_core::Error::NotFound(_) => PolicyHttpError::NotFound,
            egret_core::Error::Conflict(s) => PolicyHttpError::Conflict(s),
            egret_core::Error::InvalidInput(s) => PolicyHttpError::BadRequest(s),
            egret_core::Error::Unauthorized | egret_core::Error::Forbidden => {
                PolicyHttpError::Forbidden
            }
            // Exhaustiveness only: this seam serves `/api/admin/*` policy
            // routes, none of which go through `WsContext`, so the gate that
            // produces `FeatureDisabled` can never actually reach here.
            egret_core::Error::FeatureDisabled(_) => PolicyHttpError::Forbidden,
            egret_core::Error::TooManyRequests => {
                PolicyHttpError::BadRequest("too many requests".to_string())
            }
            egret_core::Error::Internal(_) => PolicyHttpError::Internal,
        }
    }
}

impl From<EngineReadError> for PolicyHttpError {
    fn from(_: EngineReadError) -> Self {
        PolicyHttpError::Internal
    }
}

impl IntoResponse for PolicyHttpError {
    fn into_response(self) -> Response {
        let (status, msg) = match &self {
            PolicyHttpError::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
            PolicyHttpError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            PolicyHttpError::AlreadyExists(_) => (StatusCode::CONFLICT, self.to_string()),
            PolicyHttpError::BadRequest(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            PolicyHttpError::Conflict(_) => (StatusCode::CONFLICT, self.to_string()),
            PolicyHttpError::Overlap { body } => {
                return (StatusCode::CONFLICT, Json(body.clone())).into_response()
            }
            PolicyHttpError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            ),
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}
