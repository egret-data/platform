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
    /// An unexpected failure below the domain — a query-engine error, a
    /// credential broker failure, or anything else that is not the caller's
    /// fault to fix. The real message is logged; the response is redacted the
    /// same way `egret_core::Error::Internal` redacts, so an engine's error
    /// text (which can carry table/column names) never reaches the browser.
    #[error("internal: {0}")]
    Internal(String),
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
            // Fix-round-1, Minor 4: `dataapi_endpoint_id` is a plain FK
            // (`schedules_dataapi_endpoint_id_fkey`, migration 0127) with no
            // Rust-side existence check — none of the four CHECKs this module
            // otherwise mirrors cover "does this uuid name a real row", only
            // its SHAPE. A caller authorized by a workspace-cascaded grant
            // (a `ws_admin`'s `data_api_endpoint:read` satisfies the object
            // check for ANY id, real or not — Cedar's `resource in ?resource`
            // never looks the row up) can otherwise turn a typo'd uuid into a
            // 500 instead of a 400 naming the field.
            Some("schedules_dataapi_endpoint_id_fkey") => AutomationError::BadRequest(
                "dataapi_endpoint_id does not name an existing endpoint".into(),
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
            AutomationError::Internal(msg) => {
                tracing::error!(error = %msg, "automation: internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal error".to_string(),
                )
            }
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
