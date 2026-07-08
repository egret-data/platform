//! Control-plane HTTP error type.
//!
//! Maps the platform-admin guard failure plus the underlying `CatalogError` /
//! `SecretsError` domain errors onto HTTP status codes.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use egret_catalog::CatalogError;
use egret_policy::AuthzError;
use egret_secrets::SecretsError;

#[derive(Debug, thiserror::Error)]
pub enum CpError {
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

impl From<CatalogError> for CpError {
    fn from(e: CatalogError) -> Self {
        match e {
            CatalogError::NotFound(_) => CpError::NotFound,
            CatalogError::AlreadyExists(s) => CpError::AlreadyExists(s),
            CatalogError::InUse => CpError::BadRequest(
                "role is in use by one or more Catalog Clients — delete those clients first"
                    .to_string(),
            ),
            CatalogError::BadRequest(s) => CpError::BadRequest(s),
            CatalogError::Db(_) => CpError::Internal,
        }
    }
}

impl From<egret_engine_mgmt::EngineMgmtError> for CpError {
    fn from(e: egret_engine_mgmt::EngineMgmtError) -> Self {
        match e {
            egret_engine_mgmt::EngineMgmtError::AlreadyExists(s) => CpError::AlreadyExists(s),
            egret_engine_mgmt::EngineMgmtError::Db(_) => CpError::Internal,
        }
    }
}

impl From<SecretsError> for CpError {
    fn from(e: SecretsError) -> Self {
        match e {
            SecretsError::NotFound => CpError::NotFound,
            SecretsError::AlreadyExists(s) => CpError::AlreadyExists(s),
            SecretsError::InUse(n) => CpError::InUse(n),
            SecretsError::NotConfigured => {
                CpError::BadRequest("secrets master_key is not configured".to_string())
            }
            SecretsError::Crypto(_) | SecretsError::Db(_) => CpError::Internal,
        }
    }
}

impl From<AuthzError> for CpError {
    fn from(e: AuthzError) -> Self {
        match e {
            AuthzError::NotFound(_) => CpError::NotFound,
            AuthzError::AlreadyExists(s) => CpError::AlreadyExists(s),
            AuthzError::BadRequest(s) => CpError::BadRequest(s),
            AuthzError::Db(_) | AuthzError::Json(_) => CpError::Internal,
        }
    }
}

/// `egret_core::Error` is what the identity/RBAC repos (`UserRepo`, `GroupRepo`,
/// `RoleRepo`) return, used by the `access` module.
impl From<egret_core::Error> for CpError {
    fn from(e: egret_core::Error) -> Self {
        match e {
            egret_core::Error::NotFound(_) => CpError::NotFound,
            egret_core::Error::Conflict(s) => CpError::Conflict(s),
            egret_core::Error::InvalidInput(s) => CpError::BadRequest(s),
            egret_core::Error::Unauthorized | egret_core::Error::Forbidden => CpError::Forbidden,
            egret_core::Error::TooManyRequests => {
                CpError::BadRequest("too many requests".to_string())
            }
            egret_core::Error::Internal(_) => CpError::Internal,
        }
    }
}

impl IntoResponse for CpError {
    fn into_response(self) -> Response {
        let (status, msg) = match &self {
            CpError::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
            CpError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            CpError::AlreadyExists(_) => (StatusCode::CONFLICT, self.to_string()),
            CpError::BadRequest(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            CpError::Conflict(_) => (StatusCode::CONFLICT, self.to_string()),
            CpError::InUse(_) => (StatusCode::CONFLICT, self.to_string()),
            CpError::BadGateway => (StatusCode::BAD_GATEWAY, self.to_string()),
            CpError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            ),
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}
