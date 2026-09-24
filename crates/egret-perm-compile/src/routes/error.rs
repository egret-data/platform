//! HTTP error for the `perm` router. Same status codes and `{"error": msg}`
//! body shape as the Policy/Ranger seam's `PolicyHttpError`, so the two
//! consoles under `/api/control-plane` behave identically to a client.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

use egret_perm::{CompileError, PermError};
use egret_policy::PolicyStoreError;

#[derive(Debug, thiserror::Error)]
pub enum PermHttpError {
    /// Authenticated but lacks the required `policy:*` permission.
    #[error("forbidden")]
    Forbidden,
    #[error("not found")]
    NotFound,
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("internal error")]
    Internal,
}

impl PermHttpError {
    /// A database failure the handler itself hit (`begin`, `commit`,
    /// `acquire`). Logged, not echoed: the message can name schema internals.
    pub(crate) fn db<E: std::fmt::Display>(e: E) -> Self {
        tracing::error!(error = %e, "perm route database error");
        PermHttpError::Internal
    }
}

impl From<PermError> for PermHttpError {
    fn from(e: PermError) -> Self {
        match e {
            PermError::NotFound(_) => PermHttpError::NotFound,
            PermError::Invalid(s) => {
                tracing::error!(error = %s, "perm store returned data it cannot parse");
                PermHttpError::Internal
            }
            PermError::Db(e) => PermHttpError::db(e),
        }
    }
}

impl From<PolicyStoreError> for PermHttpError {
    fn from(e: PolicyStoreError) -> Self {
        match e {
            PolicyStoreError::NotFound(_) => PermHttpError::NotFound,
            PolicyStoreError::BadRequest(s) => PermHttpError::BadRequest(s),
            other => PermHttpError::db(other),
        }
    }
}

/// **The mapping that carries the task's point.** A recompile failure is not
/// an internal error to be swallowed — it is the reason the request fails and
/// the write rolls back, so the response has to say which of the two kinds it
/// was:
///
/// - Every variant below except `Db` describes something about the grant or
///   rule THE CALLER JUST WROTE that cannot be expressed against a profile it
///   affects (an undeclared mask type, an unparseable custom mask, a `Role` in
///   an exemption, an expansion over the ceiling). That is a 400 naming the
///   refusal, because the author can fix it — and because a 500 would read as
///   "egret is broken" for an input egret correctly refused.
/// - `Db` is the genuine internal failure: 500, message withheld.
///
/// `UnexpandedPrincipalKind` never reaches here from the recompile driver — it
/// is recorded as a skip and surfaced in the response's `skipped` list instead
/// — but it is mapped as a 400 for exhaustiveness, since a direct caller of
/// the pure compiler would get it for an input the author wrote.
impl From<CompileError> for PermHttpError {
    fn from(e: CompileError) -> Self {
        match e {
            CompileError::Db(detail) => PermHttpError::db(detail),
            CompileError::ServiceDefUnreadable { .. } => {
                tracing::error!(error = %e, "embedded service-def failed to parse");
                PermHttpError::Internal
            }
            other => PermHttpError::BadRequest(other.to_string()),
        }
    }
}

/// `egret_core::Error` is what `egret_core::authz::require` returns.
impl From<egret_core::Error> for PermHttpError {
    fn from(e: egret_core::Error) -> Self {
        match e {
            egret_core::Error::NotFound(_) => PermHttpError::NotFound,
            egret_core::Error::InvalidInput(s) | egret_core::Error::Conflict(s) => {
                PermHttpError::BadRequest(s)
            }
            egret_core::Error::Unauthorized | egret_core::Error::Forbidden => {
                PermHttpError::Forbidden
            }
            egret_core::Error::FeatureDisabled(_) => PermHttpError::Forbidden,
            egret_core::Error::TooManyRequests => {
                PermHttpError::BadRequest("too many requests".to_string())
            }
            egret_core::Error::Internal(s) => PermHttpError::db(s),
        }
    }
}

impl IntoResponse for PermHttpError {
    fn into_response(self) -> Response {
        let (status, msg) = match &self {
            PermHttpError::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
            PermHttpError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            PermHttpError::BadRequest(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            PermHttpError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            ),
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}
