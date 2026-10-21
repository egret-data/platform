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
    /// 403 with a sentence saying why: the caller holds the permission the
    /// route checks, but the object itself is closed to them.
    #[error("{0}")]
    ForbiddenBecause(String),
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
    /// A tag write (rename, classification) would create an order-ambiguous
    /// mask or row-filter pair. 409 with the structured body the perm routes
    /// answer (`code: "rule_overlap"`, `rules`, `coordinate`, `kind`), which the
    /// console parses to name the two rules.
    #[error("rule overlap: {0}")]
    RuleOverlap(serde_json::Value),
    /// A refusal with a stable `code`: the body (`{error, code, ...}`) is
    /// served as is with the status.
    #[error("{}", .1["error"].as_str().unwrap_or("refused"))]
    Coded(StatusCode, serde_json::Value),
    #[error("internal error")]
    Internal,
    /// A dependency egret itself runs (its warehouse) failed. 503; the
    /// message is a sentence for the page, never the dependency's own text.
    #[error("{0}")]
    Unavailable(String),
    /// A dependency egret itself runs did not answer within its deadline.
    /// 504; the message names what and the deadline.
    #[error("{0}")]
    Timeout(String),
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
            // SP4d-4 Task 3: a rule overlap is a 409 whose sentence is `error`,
            // and whose structured body is served as is.
            E::Overlap { body } => CpError::RuleOverlap(body),
            E::Coded { status, body } => CpError::Coded(status, body),
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
        if let CpError::RuleOverlap(body) = self {
            return (StatusCode::CONFLICT, Json(body)).into_response();
        }
        if let CpError::Coded(status, body) = self {
            return (status, Json(body)).into_response();
        }
        let (status, msg) = match &self {
            CpError::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
            CpError::ForbiddenBecause(_) => (StatusCode::FORBIDDEN, self.to_string()),
            CpError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            CpError::AlreadyExists(_) => (StatusCode::CONFLICT, self.to_string()),
            CpError::BadRequest(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            CpError::Conflict(_) => (StatusCode::CONFLICT, self.to_string()),
            CpError::InUse(_) => (StatusCode::CONFLICT, self.to_string()),
            CpError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            ),
            CpError::Unavailable(_) => (StatusCode::SERVICE_UNAVAILABLE, self.to_string()),
            CpError::Timeout(_) => (StatusCode::GATEWAY_TIMEOUT, self.to_string()),
            CpError::RuleOverlap(_) => unreachable!("answered above with its structured body"),
            CpError::Coded(..) => unreachable!("answered above with its structured body"),
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    #[tokio::test]
    async fn a_tag_write_overlap_keeps_its_structured_409_body() {
        let body = serde_json::json!({
            "error": "assigning this tag would create an order-ambiguous pair",
            "code": "rule_overlap",
            "rules": [{"id": null, "name": "a"}, {"id": null, "name": "b"}],
            "coordinate": "wh.sales.orders.email",
            "kind": "mask",
        });
        let resp = CpError::from(egret_bi::BiError::Overlap { body: body.clone() }).into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let got: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(got, body);
    }
}
