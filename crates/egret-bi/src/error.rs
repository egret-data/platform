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
    /// Two mask (or row-filter) policies would decide one request differently
    /// — a tag write's recompile refused, or the write created such a pair
    /// among the stored policies (SP4d-4 Task 3). 409 with
    /// `egret_perm_compile::overlap::refusal_body`, served as is, so the tag
    /// UI gets the same `{error, code: "rule_overlap", rules, coordinate,
    /// kind}` the rule routes answer.
    #[error("rule overlap: {body}")]
    Overlap { body: serde_json::Value },
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

impl From<egret_query::DatabaseError> for BiError {
    fn from(e: egret_query::DatabaseError) -> Self {
        match e {
            egret_query::DatabaseError::NotFound(s) => BiError::NotFound(s),
            other => BiError::Engine(other.to_string()),
        }
    }
}
impl From<egret_query::EngineError> for BiError {
    fn from(e: egret_query::EngineError) -> Self {
        match e {
            egret_query::EngineError::NotFound(s) => BiError::NotFound(s),
            other => BiError::Engine(other.to_string()),
        }
    }
}

/// SP4a Task 7: a catalog-tag write recompiles every StarRocks profile it can
/// stale (D5), and a genuine `CompileError` (an authoring/resolution bug —
/// `WorkspaceMembers` is skipped, not surfaced as an error; see
/// `egret-perm-compile::trigger`'s module doc) must not disappear into an
/// opaque 500. `Engine` already means "a downstream policy/engine operation
/// failed" and renders the message, not just a generic label.
impl From<egret_perm::CompileError> for BiError {
    fn from(e: egret_perm::CompileError) -> Self {
        match egret_perm_compile::overlap::refusal_body(&e) {
            Some(body) => BiError::Overlap { body },
            None => BiError::Engine(format!("permission recompile failed: {e}")),
        }
    }
}

/// Domain arm only — the shared `Auth` arm is in `egret_core::ApiError`.
impl IntoResponse for BiError {
    fn into_response(self) -> Response {
        let msg = self.to_string();
        let error = |m: &str| serde_json::json!({ "error": m });
        let (status, body) = match self {
            BiError::NotFound(_) => (StatusCode::NOT_FOUND, error(&msg)),
            BiError::Forbidden => (StatusCode::FORBIDDEN, error("forbidden")),
            BiError::BadRequest(_) => (StatusCode::BAD_REQUEST, error(&msg)),
            BiError::Conflict(_) => (StatusCode::CONFLICT, error(&msg)),
            BiError::Engine(_) => (StatusCode::BAD_GATEWAY, error(&msg)),
            // The structured body, served as is: its `error` is the sentence.
            BiError::Overlap { body } => (StatusCode::CONFLICT, body),
            BiError::Db(_) => (StatusCode::INTERNAL_SERVER_ERROR, error("internal error")),
        };
        (status, Json(body)).into_response()
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
