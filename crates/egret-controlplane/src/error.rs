//! Control-plane HTTP error type.
//!
//! Maps the platform-admin guard failure plus the underlying `CatalogError` /
//! `SecretsError` domain errors onto HTTP status codes.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use egret_catalog_engine::CatalogError;
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
            CatalogError::Conflict(s) => CpError::Conflict(s),
            CatalogError::Db(_) => CpError::Internal,
            // `delete_warehouse_releasing_catalog`'s in-transaction recompile
            // (SP4a Task 16, D3 defect 3) failing — same `Internal` treatment
            // as `Db`.
            CatalogError::Compile(_) => CpError::Internal,
            CatalogError::Perm(_) => CpError::Internal,
        }
    }
}

/// A recompile that accompanies a control-plane write failing — SP4c Task 3's
/// role-membership path. Same `Internal` treatment `CatalogError::Compile`
/// already gets, and for the same reason: the transaction rolled the write back
/// with it, so the caller's state is unchanged and there is nothing they can fix
/// by editing their request.
impl From<egret_perm::CompileError> for CpError {
    fn from(e: egret_perm::CompileError) -> Self {
        tracing::error!(error = %e, "a control-plane write's policy recompile failed");
        CpError::Internal
    }
}

impl From<egret_storage::StorageRegistryError> for CpError {
    fn from(e: egret_storage::StorageRegistryError) -> Self {
        use egret_storage::StorageRegistryError as E;
        match e {
            E::NotFound(_) => CpError::NotFound,
            E::AlreadyExists(s) => CpError::AlreadyExists(s),
            E::Db(_) => CpError::Internal,
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
            SecretsError::InvalidType(_) | SecretsError::MissingKey(_) => {
                CpError::BadRequest(e.to_string())
            }
            SecretsError::Crypto(_) | SecretsError::Db(_) => CpError::Internal,
        }
    }
}

/// `egret_bi::BiError` is what `TagRepo` (the `tags` module) returns.
impl From<egret_bi::BiError> for CpError {
    fn from(e: egret_bi::BiError) -> Self {
        use egret_bi::BiError as E;
        match e {
            E::NotFound(_) => CpError::NotFound,
            E::Forbidden => CpError::Forbidden,
            E::BadRequest(s) => CpError::BadRequest(s),
            E::Conflict(s) => CpError::Conflict(s),
            // SP4d-4 Task 3: a rule overlap is a 409 whose sentence is `error`.
            E::Overlap { body } => CpError::Conflict(
                body["error"]
                    .as_str()
                    .unwrap_or("two mask or row-filter rules overlap")
                    .to_string(),
            ),
            E::Engine(_) | E::Db(_) => CpError::Internal,
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
            // Exhaustiveness only: this seam serves `/api/control-plane/*`
            // routes, none of which go through `WsContext`, so the gate that
            // produces `FeatureDisabled` can never actually reach here.
            egret_core::Error::FeatureDisabled(_) => CpError::Forbidden,
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
            CpError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            ),
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}
