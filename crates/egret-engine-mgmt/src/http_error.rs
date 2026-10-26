//! Engine-mgmt seam HTTP error.
//!
//! The variants that predate the engine-delete preflight mirror
//! `egret-controlplane`'s mapping exactly (status + `{"error": msg}` body). The
//! three added for engine delete/unassign also carry a machine-readable `code`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use egret_catalog_api::CatalogApiError;
use egret_secrets::SecretsError;

use crate::dependents::{EngineConflictBody, EngineDependentDto};

#[derive(Debug, thiserror::Error)]
pub enum EngineHttpError {
    /// Caller is authenticated but lacks the `platform_admin` role.
    #[error("forbidden")]
    Forbidden,
    #[error("not found")]
    NotFound,
    /// A 404 that says WHAT was not found, where the id in the message is the
    /// only way the caller can tell which of several it was.
    #[error("not found: {0}")]
    NotFoundNamed(String),
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
    /// Data API endpoints, schedules or datasets still use the engine's workspace
    /// connections; nothing was changed. 409 with the list.
    #[error("engine is still in use")]
    HasDependents(Vec<EngineDependentDto>),
    /// A write hit a foreign-key (23503) or CHECK (23514) constraint the
    /// preflight did not anticipate. 409 naming the constraint.
    #[error("blocked by database constraint {0}")]
    ConstraintViolation(String),
    /// The Kubernetes delete failed with something other than 404. The engine row
    /// stays `deleting` and the delete can be retried. 502 with the cause.
    #[error("engine cluster delete failed: {0}")]
    ClusterDeleteFailed(String),
    /// Kubernetes rejected (or never answered) an apply of an engine's rendered
    /// spec. The stored state was restored; the cause is surfaced verbatim.
    #[error("engine cluster apply failed: {0}")]
    ClusterApplyFailed(String),
    /// A registry catalog is still attached to engines or assigned to
    /// workspaces; nothing was changed. 409 listing both.
    #[error("catalog is still in use")]
    CatalogInUse {
        engines: Vec<crate::catalogs::CatalogConflictEngine>,
        workspaces: Vec<crate::catalogs::CatalogConflictWorkspace>,
    },
    /// A registry catalog is an egret warehouse catalog: it goes with its
    /// warehouse and can never be deleted through `/catalogs`. 409, unconditional
    /// (checked before attachments/assignments, since telling the caller to
    /// detach/unassign it would be pointless — it stays refused either way).
    #[error("catalog is an egret warehouse")]
    WarehouseCatalog(String),
    /// The engine is external: egret holds a row for it and never provisions,
    /// updates, restarts, scales, execs into or deletes the cluster of it, so a
    /// lifecycle route refuses (SP4d-4 Part C). 409, `code: "engine_is_external"`.
    #[error("engine is external: {0}")]
    EngineIsExternal(String),
    /// The write would create an order-ambiguous mask or row-filter pair
    /// (`CompileError::AmbiguousMaskOrFilterArbitration`). 409 with the body
    /// the perm routes answer (`code: "rule_overlap"`).
    #[error("rule overlap: {0}")]
    RuleOverlap(serde_json::Value),
    #[error("internal error")]
    Internal,
}

/// The one place a `sqlx::Error` becomes an HTTP error. It is logged here because
/// this is the last point the error still exists: `Internal` has no payload, and
/// "internal error with nothing in the logs" was the reported defect.
/// `pub(crate)`: `catalogs::set_catalog_workspaces` maps `sqlx::Error` from
/// its own `.begin()`/`.commit()` through this explicitly rather than
/// through a blanket `From<sqlx::Error> for EngineHttpError` (Task 6 fix
/// round 1, Minor) — a crate-wide `impl From` would make every OTHER raw
/// sqlx `?` in this crate compile through the same mapping with no
/// deliberate decision at the call site, which is not a promise this crate
/// makes anywhere else (every other `sqlx::Error` here is wrapped in a
/// seam-specific error type first).
pub(crate) fn db_error(e: sqlx::Error) -> EngineHttpError {
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
        "engine-mgmt database error"
    );
    match (sqlstate.as_deref(), constraint) {
        (Some("23503") | Some("23514"), Some(name)) => EngineHttpError::ConstraintViolation(name),
        _ => EngineHttpError::Internal,
    }
}

impl From<CatalogApiError> for EngineHttpError {
    fn from(e: CatalogApiError) -> Self {
        match e {
            CatalogApiError::NotFound(_) => EngineHttpError::NotFound,
            CatalogApiError::AlreadyExists(s) => EngineHttpError::AlreadyExists(s),
            CatalogApiError::InUse => EngineHttpError::BadRequest(
                "role is in use by one or more Catalog Clients — delete those clients first"
                    .to_string(),
            ),
            CatalogApiError::BadRequest(s) => EngineHttpError::BadRequest(s),
            CatalogApiError::Backend(_) => EngineHttpError::Internal,
        }
    }
}

impl From<egret_storage::StorageRegistryError> for EngineHttpError {
    fn from(e: egret_storage::StorageRegistryError) -> Self {
        use egret_storage::StorageRegistryError as E;
        match e {
            E::NotFound(_) => EngineHttpError::NotFound,
            E::AlreadyExists(s) => EngineHttpError::AlreadyExists(s),
            E::Db(e) => db_error(e),
        }
    }
}

impl From<crate::EngineMgmtError> for EngineHttpError {
    fn from(e: crate::EngineMgmtError) -> Self {
        match e {
            crate::EngineMgmtError::AlreadyExists(s) => EngineHttpError::AlreadyExists(s),
            crate::EngineMgmtError::NotFound(_) => EngineHttpError::NotFound,
            crate::EngineMgmtError::NotAssignable(s) => EngineHttpError::BadRequest(s),
            crate::EngineMgmtError::Db(e) => db_error(e),
        }
    }
}

impl From<egret_catalog_registry::RegistryError> for EngineHttpError {
    fn from(e: egret_catalog_registry::RegistryError) -> Self {
        use egret_catalog_registry::RegistryError as E;
        match e {
            E::NotFound(_) => EngineHttpError::NotFound,
            E::AlreadyExists(s) => EngineHttpError::AlreadyExists(s),
            E::Conflict(s) => EngineHttpError::Conflict(s),
            E::Invalid(v) => EngineHttpError::BadRequest(v.to_string()),
            E::Secrets(s) => s.into(),
            E::Db(e) => db_error(e),
            // `ensure_warehouse_catalog`'s implicit-grant sync (Task 6 fix
            // round 1, Important 1) failing inside its own transaction.
            E::Perm(e) => e.into(),
            // `ensure_warehouse_catalog_with_compiler`'s in-transaction
            // recompile (SP4a Task 16, D3 defect 2) failing — same treatment
            // as `E::Perm` above, through the same `CompileError` mapping
            // `catalogs::set_catalog_workspaces` already uses.
            E::Compile(e) => e.into(),
        }
    }
}

impl From<egret_perm::PermError> for EngineHttpError {
    fn from(e: egret_perm::PermError) -> Self {
        use egret_perm::PermError as E;
        match e {
            E::NotFound(_) => EngineHttpError::NotFound,
            E::Invalid(s) => EngineHttpError::BadRequest(s),
            E::Db(e) => db_error(e),
        }
    }
}

/// `catalogs::set_catalog_workspaces`'s in-transaction recompile (SP4a Task
/// 15, D3 defect 1) failing. Mapped to `Internal` rather than surfaced
/// verbatim, the same treatment `db_error` gives a raw `sqlx::Error` here —
/// but, unlike `ProfileCompilerPort::recompile_profile`'s fire-and-forget
/// callers, this one is reached through `?` and DOES fail (and roll back)
/// the request: a grant written but not compiled is worse than a rejected
/// write.
impl From<egret_perm::CompileError> for EngineHttpError {
    fn from(e: egret_perm::CompileError) -> Self {
        // A pair the write CREATED is the caller's to fix, not a server fault.
        if let Some(body) = e.rule_overlap_body() {
            return EngineHttpError::RuleOverlap(body);
        }
        // So is a ceiling the write would cross (D2, U3).
        if e.expansion_ceiling_body().is_some() {
            return EngineHttpError::Conflict(e.to_string());
        }
        tracing::error!(
            error = %e,
            "perm-model recompile failed inside a catalog-workspace assignment write"
        );
        EngineHttpError::Internal
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
            SecretsError::InvalidType(_) | SecretsError::MissingKey(_) => {
                EngineHttpError::BadRequest(e.to_string())
            }
            SecretsError::Crypto(_) => EngineHttpError::Internal,
            SecretsError::Db(e) => db_error(e),
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
            // Exhaustiveness only: this seam serves `/api/control-plane/*`
            // engine routes, none of which go through `WsContext`, so the gate
            // that produces `FeatureDisabled` can never actually reach here.
            egret_core::Error::FeatureDisabled(_) => EngineHttpError::Forbidden,
            egret_core::Error::TooManyRequests => {
                EngineHttpError::BadRequest("too many requests".to_string())
            }
            egret_core::Error::Internal(_) => EngineHttpError::Internal,
        }
    }
}

fn error_json(status: StatusCode, msg: String) -> Response {
    (status, Json(serde_json::json!({ "error": msg }))).into_response()
}

impl IntoResponse for EngineHttpError {
    fn into_response(self) -> Response {
        let msg = self.to_string();
        match self {
            EngineHttpError::HasDependents(dependents) => {
                let workspaces = dependents
                    .iter()
                    .map(|d| d.workspace_id)
                    .collect::<std::collections::BTreeSet<_>>()
                    .len();
                let body = EngineConflictBody {
                    error: format!(
                        "{msg}: {workspaces} workspace(s) have Data API endpoints, alerts or \
                         datasets on it; remove or move them first"
                    ),
                    code: "engine_has_dependents".into(),
                    dependents,
                    constraint: None,
                };
                (StatusCode::CONFLICT, Json(body)).into_response()
            }
            EngineHttpError::ConstraintViolation(constraint) => {
                let body = EngineConflictBody {
                    error: msg,
                    code: "constraint_violation".into(),
                    dependents: Vec::new(),
                    constraint: Some(constraint),
                };
                (StatusCode::CONFLICT, Json(body)).into_response()
            }
            EngineHttpError::ClusterDeleteFailed(_) => (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": msg, "code": "engine_cluster_delete_failed" })),
            )
                .into_response(),
            EngineHttpError::ClusterApplyFailed(_) => (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": msg, "code": "engine_cluster_apply_failed" })),
            )
                .into_response(),
            EngineHttpError::CatalogInUse {
                engines,
                workspaces,
            } => {
                let body = crate::catalogs::CatalogConflictBody {
                    error: format!(
                        "{msg}: attached to {} engine(s) and assigned to {} workspace(s); \
                         detach and unassign it first",
                        engines.len(),
                        workspaces.len()
                    ),
                    code: "catalog_in_use".into(),
                    engines,
                    workspaces,
                };
                (StatusCode::CONFLICT, Json(body)).into_response()
            }
            EngineHttpError::WarehouseCatalog(error) => {
                let body = crate::catalogs::CatalogConflictBody {
                    error,
                    code: "warehouse_catalog".into(),
                    engines: Vec::new(),
                    workspaces: Vec::new(),
                };
                (StatusCode::CONFLICT, Json(body)).into_response()
            }
            EngineHttpError::EngineIsExternal(reason) => (
                StatusCode::CONFLICT,
                Json(serde_json::json!({ "error": reason, "code": "engine_is_external" })),
            )
                .into_response(),
            EngineHttpError::RuleOverlap(body) => {
                (StatusCode::CONFLICT, Json(body)).into_response()
            }
            EngineHttpError::Forbidden => error_json(StatusCode::FORBIDDEN, msg),
            EngineHttpError::NotFound | EngineHttpError::NotFoundNamed(_) => {
                error_json(StatusCode::NOT_FOUND, msg)
            }
            EngineHttpError::AlreadyExists(_)
            | EngineHttpError::Conflict(_)
            | EngineHttpError::InUse(_) => error_json(StatusCode::CONFLICT, msg),
            EngineHttpError::BadRequest(_) => error_json(StatusCode::BAD_REQUEST, msg),
            EngineHttpError::BadGateway => error_json(StatusCode::BAD_GATEWAY, msg),
            EngineHttpError::Internal => error_json(StatusCode::INTERNAL_SERVER_ERROR, msg),
        }
    }
}

#[cfg(test)]
mod tests {
    /// Live check F1: a catalog-workspace write whose recompile crosses an
    /// expansion ceiling is the caller's 409 with the compiler's sentence.
    #[test]
    fn a_ceiling_refusal_is_a_conflict() {
        let e = egret_perm::CompileError::ExpansionCeiling {
            rule: "r".into(),
            count: 501,
            ceiling: 500,
        };
        let sentence = e.to_string();
        assert!(matches!(EngineHttpError::from(e), EngineHttpError::Conflict(m) if m == sentence));
    }

    use super::*;
    use crate::dependents::EngineDependentKind;

    async fn body(resp: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn dependents_are_a_409_carrying_the_list() {
        let resp = EngineHttpError::HasDependents(vec![EngineDependentDto {
            workspace_id: uuid::Uuid::from_u128(1),
            workspace_name: "Sales".into(),
            workspace_slug: "sales".into(),
            kind: EngineDependentKind::DataapiEndpointTrashed,
            count: 1,
            sample: vec!["old-orders".into()],
        }])
        .into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let j = body(resp).await;
        assert_eq!(j["code"], "engine_has_dependents");
        assert_eq!(j["dependents"][0]["workspaceSlug"], "sales");
        assert_eq!(j["dependents"][0]["kind"], "dataapi_endpoint_trashed");
        assert!(
            j["error"].as_str().unwrap().contains("1 workspace(s)"),
            "{j}"
        );
        assert!(j.get("constraint").is_none(), "{j}");
    }

    #[tokio::test]
    async fn a_failed_cluster_delete_is_a_502_with_its_cause() {
        let resp =
            EngineHttpError::ClusterDeleteFailed("Deployment/x: forbidden".into()).into_response();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(
            body(resp).await,
            serde_json::json!({
                "error": "engine cluster delete failed: Deployment/x: forbidden",
                "code": "engine_cluster_delete_failed"
            })
        );
    }

    #[tokio::test]
    async fn existing_variants_keep_their_bodies() {
        let resp = EngineHttpError::BadRequest("kubernetes not configured".into()).into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            body(resp).await,
            serde_json::json!({ "error": "bad request: kubernetes not configured" })
        );
        let resp = EngineHttpError::NotFound.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            body(resp).await,
            serde_json::json!({ "error": "not found" })
        );
        let resp = EngineHttpError::InUse(2).into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        assert_eq!(
            body(resp).await,
            serde_json::json!({ "error": "secret is in use by 2 connection(s)" })
        );
    }
}
