//! Engine-mgmt seam HTTP error.
//!
//! Mirrors `egret-controlplane`'s `CpError` mapping exactly (status codes +
//! bodies unchanged) so the moved `engines.rs`/`monitor.rs` handlers keep
//! behaving identically after the relocation.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use egret_authz::AuthzError;
use egret_catalog::CatalogError;
use egret_secrets::SecretsError;

#[derive(Debug, thiserror::Error)]
pub enum EngineHttpError {
    /// Caller is authenticated but lacks the `platform_admin` role.
    #[error("forbidden")]
    Forbidden,
    #[error("not found")]
    NotFound,
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    /// A state conflict that isn't a duplicate-name/email clash covered by
    /// `AlreadyExists` — e.g. the last-platform-admin guard or a seeded-role
    /// delete guard.
    #[error("conflict: {0}")]
    Conflict(String),
    /// A named secret is still referenced and cannot be deleted.
    #[error("secret is in use by {0} connection(s)")]
    InUse(i64),
    /// A Trino coordinator request failed (unreachable or non-2xx / non-JSON
    /// response) — surfaced to the UI as a 502 the monitor page can show.
    #[error("engine not reachable")]
    BadGateway,
    #[error("internal error")]
    Internal,
}

impl From<CatalogError> for EngineHttpError {
    fn from(e: CatalogError) -> Self {
        match e {
            CatalogError::NotFound(_) => EngineHttpError::NotFound,
            CatalogError::AlreadyExists(s) => EngineHttpError::AlreadyExists(s),
            CatalogError::InUse => EngineHttpError::BadRequest(
                "role is in use by one or more Catalog Clients — delete those clients first"
                    .to_string(),
            ),
            CatalogError::BadRequest(s) => EngineHttpError::BadRequest(s),
            CatalogError::Db(_) => EngineHttpError::Internal,
        }
    }
}

impl From<crate::EngineMgmtError> for EngineHttpError {
    fn from(e: crate::EngineMgmtError) -> Self {
        match e {
            crate::EngineMgmtError::AlreadyExists(s) => EngineHttpError::AlreadyExists(s),
            crate::EngineMgmtError::Db(_) => EngineHttpError::Internal,
        }
    }
}

impl From<SecretsError> for EngineHttpError {
    fn from(e: SecretsError) -> Self {
        match e {
            SecretsError::NotFound => EngineHttpError::NotFound,
            SecretsError::AlreadyExists(s) => EngineHttpError::AlreadyExists(s),
            SecretsError::InUse(n) => EngineHttpError::InUse(n),
            SecretsError::NotConfigured => {
                EngineHttpError::BadRequest("secrets master_key is not configured".to_string())
            }
            SecretsError::Crypto(_) | SecretsError::Db(_) => EngineHttpError::Internal,
        }
    }
}

impl From<AuthzError> for EngineHttpError {
    fn from(e: AuthzError) -> Self {
        match e {
            AuthzError::NotFound(_) => EngineHttpError::NotFound,
            AuthzError::AlreadyExists(s) => EngineHttpError::AlreadyExists(s),
            AuthzError::BadRequest(s) => EngineHttpError::BadRequest(s),
            AuthzError::Db(_) | AuthzError::Json(_) => EngineHttpError::Internal,
        }
    }
}

/// `egret_core::Error` is what `require_permission` (from `egret-auth`, post-P5)
/// returns.
impl From<egret_core::Error> for EngineHttpError {
    fn from(e: egret_core::Error) -> Self {
        match e {
            egret_core::Error::NotFound(_) => EngineHttpError::NotFound,
            egret_core::Error::Conflict(s) => EngineHttpError::Conflict(s),
            egret_core::Error::InvalidInput(s) => EngineHttpError::BadRequest(s),
            egret_core::Error::Unauthorized | egret_core::Error::Forbidden => {
                EngineHttpError::Forbidden
            }
            egret_core::Error::TooManyRequests => {
                EngineHttpError::BadRequest("too many requests".to_string())
            }
            egret_core::Error::Internal(_) => EngineHttpError::Internal,
        }
    }
}

impl IntoResponse for EngineHttpError {
    fn into_response(self) -> Response {
        let (status, msg) = match &self {
            EngineHttpError::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
            EngineHttpError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            EngineHttpError::AlreadyExists(_) => (StatusCode::CONFLICT, self.to_string()),
            EngineHttpError::BadRequest(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            EngineHttpError::Conflict(_) => (StatusCode::CONFLICT, self.to_string()),
            EngineHttpError::InUse(_) => (StatusCode::CONFLICT, self.to_string()),
            EngineHttpError::BadGateway => (StatusCode::BAD_GATEWAY, self.to_string()),
            EngineHttpError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            ),
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}
