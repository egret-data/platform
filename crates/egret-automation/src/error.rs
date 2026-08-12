//! Automation domain error + its HTTP mapping, following `egret-bi`'s
//! `BiError`/`BiApiError` pair: the shared auth/validation arm lives in
//! `egret_core::ApiError`, and this file carries only the domain arm.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use egret_core::ApiError;

#[derive(Debug, thiserror::Error)]
pub enum AutomationError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

impl AutomationError {
    /// Map a database error to the domain, turning the two constraint families
    /// the write path can legitimately hit into 4xx.
    ///
    /// Without this, a duplicate schedule name is a 500 and reads to the user
    /// as "the product is broken" rather than "pick another name". The CHECK
    /// constraints should already have been caught by route validation — if one
    /// fires here it means validation and the schema disagree, so it maps to a
    /// 400 carrying the constraint name rather than being silently smoothed
    /// over.
    pub fn from_db(e: sqlx::Error) -> Self {
        let Some(db_err) = e.as_database_error() else {
            return AutomationError::Db(e);
        };
        match db_err.constraint() {
            Some("schedule_name_uniq") => AutomationError::Conflict(
                "a schedule of this type already has that name in this workspace".into(),
            ),
            Some(c) if c.starts_with("schedule_") || c.starts_with("recipient_") => {
                AutomationError::BadRequest(format!("violates {c}"))
            }
            _ => AutomationError::Db(e),
        }
    }
}

impl From<crate::cron::CronError> for AutomationError {
    fn from(e: crate::cron::CronError) -> Self {
        AutomationError::BadRequest(e.to_string())
    }
}

impl IntoResponse for AutomationError {
    fn into_response(self) -> Response {
        let (status, msg) = match &self {
            AutomationError::NotFound(_) => (StatusCode::NOT_FOUND, self.to_string()),
            AutomationError::BadRequest(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            AutomationError::Conflict(_) => (StatusCode::CONFLICT, self.to_string()),
            AutomationError::Db(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            ),
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

/// The automation handler error type. `From<egret_core::Error>` comes from
/// core; this adds the domain-side `From` so `?` works on `AutomationError`.
pub type AutomationApiError = ApiError<AutomationError>;

impl From<AutomationError> for AutomationApiError {
    fn from(e: AutomationError) -> Self {
        ApiError::Domain(e)
    }
}
