//! HTTP error for the admin catalog router. Mirrors the arms `egret-controlplane`'s
//! `CpError` produced for these routes (status codes + `{"error": msg}` body
//! unchanged) so the moved handlers behave identically after the E3d-3
//! relocation. This is where the E3d-2 typed `CatalogEngineError` finally reaches
//! the wire with the right status.

use crate::{CatalogEngineError, CatalogError};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

#[derive(Debug, thiserror::Error)]
pub enum CatalogHttpError {
    /// Authenticated but lacks the required `catalog_registry:*` permission.
    #[error("forbidden")]
    Forbidden,
    #[error("not found")]
    NotFound,
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    /// A state conflict that isn't a duplicate-name clash covered by
    /// `AlreadyExists`.
    #[error("conflict: {0}")]
    Conflict(String),
    /// A named secret / role is still referenced and cannot be deleted.
    #[error("in use by {0} reference(s)")]
    InUse(i64),
    /// A lakekeeper request failed (unreachable or non-2xx) — surfaced as a 502.
    #[error("engine not reachable")]
    BadGateway,
    #[error("internal error")]
    Internal,
}

/// The merged RBAC store error (crate-local `store::CatalogError`) — copies the
/// exact arm mapping `CpError` used, including the "role is in use" 400 message.
impl From<CatalogError> for CatalogHttpError {
    fn from(e: CatalogError) -> Self {
        match e {
            CatalogError::NotFound(_) => CatalogHttpError::NotFound,
            CatalogError::AlreadyExists(s) => CatalogHttpError::AlreadyExists(s),
            CatalogError::InUse => CatalogHttpError::BadRequest(
                "role is in use by one or more Catalog Clients — delete those clients first"
                    .to_string(),
            ),
            CatalogError::BadRequest(s) => CatalogHttpError::BadRequest(s),
            CatalogError::Conflict(s) => CatalogHttpError::Conflict(s),
            CatalogError::Db(_) => CatalogHttpError::Internal,
        }
    }
}

/// Logs a raw `sqlx::Error` before it becomes an opaque `Internal` 500 —
/// without this, `E::Db`/`E::Perm` below dropped the error entirely, so an
/// admin could tell from the response ONLY that "something broke", never
/// which write or why (SP4a Task 6 fix round 2 review). `perm_grant_sync`
/// names the finer distinction that review asked this mapping to consider:
/// a warehouse's registry write succeeding while its `perm` grant sync
/// fails in the SAME transaction (`ensure_warehouse_catalog`, Important 1 of
/// round 1) is a materially different operator story than the database
/// itself being unreachable — even though both are equally un-actionable to
/// the CALLER, so the HTTP status/body stay the same `Internal` 500. This
/// crate's other silent `Db(_) => Internal` arms (`CatalogError`,
/// `StorageRegistryError`, `SecretsError` below) are a separate, larger
/// observability gap this fix does not attempt to close.
fn log_registry_db_error(e: &sqlx::Error, perm_grant_sync: bool) {
    let (sqlstate, constraint) = match e.as_database_error() {
        Some(d) => (
            d.code().map(|c| c.into_owned()),
            d.constraint().map(str::to_owned),
        ),
        None => (None, None),
    };
    tracing::error!(
        error = %e,
        sqlstate = sqlstate.as_deref().unwrap_or("-"),
        constraint = constraint.as_deref().unwrap_or("-"),
        perm_grant_sync,
        "catalog registry database error"
    );
}

/// The registry store error (SP3): a warehouse's registry catalog (creation,
/// visibility, attachment checks on delete).
impl From<egret_catalog_registry::RegistryError> for CatalogHttpError {
    fn from(e: egret_catalog_registry::RegistryError) -> Self {
        use egret_catalog_registry::RegistryError as E;
        match e {
            E::NotFound(_) => CatalogHttpError::NotFound,
            E::AlreadyExists(s) => CatalogHttpError::AlreadyExists(s),
            E::Conflict(s) => CatalogHttpError::Conflict(s),
            E::Invalid(v) => CatalogHttpError::BadRequest(v.to_string()),
            E::Secrets(s) => s.into(),
            E::Db(e) => {
                log_registry_db_error(&e, false);
                CatalogHttpError::Internal
            }
            // `ensure_warehouse_catalog`'s implicit-grant sync (SP4a Task 6
            // fix round 1, Important 1) failing inside its own transaction —
            // same HTTP treatment as `Db` (still `Internal`, not something
            // the caller's request was wrong about), but logged as its own
            // condition rather than silently folded into a generic database
            // failure (fix round 2 review).
            E::Perm(e) => {
                match e {
                    egret_perm::PermError::Db(inner) => log_registry_db_error(&inner, true),
                    other => tracing::error!(
                        error = %other,
                        "catalog registry: perm grant-sync error (non-db)"
                    ),
                }
                CatalogHttpError::Internal
            }
        }
    }
}

/// The engine-level domain error from the per-request warehouse/browse
/// operations (E3d-2). Carries lakekeeper's own status classification through to
/// the right HTTP code.
impl From<CatalogEngineError> for CatalogHttpError {
    fn from(e: CatalogEngineError) -> Self {
        match e {
            CatalogEngineError::NotFound(_) => CatalogHttpError::NotFound,
            CatalogEngineError::BadRequest(s) => CatalogHttpError::BadRequest(s),
            CatalogEngineError::Conflict(s) => CatalogHttpError::Conflict(s),
            CatalogEngineError::Upstream(_) => CatalogHttpError::BadGateway,
            CatalogEngineError::Internal(_) => CatalogHttpError::Internal,
        }
    }
}

impl From<egret_storage::StorageRegistryError> for CatalogHttpError {
    fn from(e: egret_storage::StorageRegistryError) -> Self {
        use egret_storage::StorageRegistryError as E;
        match e {
            E::NotFound(_) => CatalogHttpError::NotFound,
            E::AlreadyExists(s) => CatalogHttpError::AlreadyExists(s),
            E::Db(_) => CatalogHttpError::Internal,
        }
    }
}

impl From<egret_secrets::SecretsError> for CatalogHttpError {
    fn from(e: egret_secrets::SecretsError) -> Self {
        use egret_secrets::SecretsError as E;
        match e {
            E::NotFound => CatalogHttpError::NotFound,
            E::AlreadyExists(s) => CatalogHttpError::AlreadyExists(s),
            E::InUse(n) => CatalogHttpError::InUse(n),
            E::NotConfigured => {
                CatalogHttpError::BadRequest("secrets master_key is not configured".to_string())
            }
            E::InvalidType(_) | E::MissingKey(_) => CatalogHttpError::BadRequest(e.to_string()),
            E::Crypto(_) | E::Db(_) => CatalogHttpError::Internal,
        }
    }
}

/// `egret_core::Error` is what `egret_core::authz::require` returns (Forbidden on
/// a denied decision).
impl From<egret_core::Error> for CatalogHttpError {
    fn from(e: egret_core::Error) -> Self {
        match e {
            egret_core::Error::NotFound(_) => CatalogHttpError::NotFound,
            egret_core::Error::Conflict(s) => CatalogHttpError::Conflict(s),
            egret_core::Error::InvalidInput(s) => CatalogHttpError::BadRequest(s),
            egret_core::Error::Unauthorized | egret_core::Error::Forbidden => {
                CatalogHttpError::Forbidden
            }
            // Exhaustiveness only: this seam serves `/api/admin/*` warehouse
            // routes, none of which go through `WsContext`, so the gate that
            // produces `FeatureDisabled` can never actually reach here.
            egret_core::Error::FeatureDisabled(_) => CatalogHttpError::Forbidden,
            egret_core::Error::TooManyRequests => {
                CatalogHttpError::BadRequest("too many requests".to_string())
            }
            egret_core::Error::Internal(_) => CatalogHttpError::Internal,
        }
    }
}

impl IntoResponse for CatalogHttpError {
    fn into_response(self) -> Response {
        let (status, msg) = match &self {
            CatalogHttpError::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
            CatalogHttpError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            CatalogHttpError::AlreadyExists(_) => (StatusCode::CONFLICT, self.to_string()),
            CatalogHttpError::BadRequest(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            CatalogHttpError::Conflict(_) => (StatusCode::CONFLICT, self.to_string()),
            CatalogHttpError::InUse(_) => (StatusCode::CONFLICT, self.to_string()),
            CatalogHttpError::BadGateway => (StatusCode::BAD_GATEWAY, self.to_string()),
            CatalogHttpError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            ),
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}
