//! Crate-level (engine) domain error for the per-request warehouse/browse
//! operations that drive the embedded lakekeeper.
//!
//! Distinct from the merged RBAC store's [`CatalogError`](crate::CatalogError)
//! (in `src/store/error.rs`): that one models the egret-side `warehouses`/roles/
//! grants store, while this one models the *engine* request path (browse /
//! admin / recovery / provisioning) so a "namespace not found" or a bad
//! request surfaces as a 404/400 rather than an opaque 500.
//!
//! Its `IntoResponse` mirrors `egret-controlplane`'s `CpError` / engine-mgmt's
//! `EngineHttpError` (status codes + `{"error": msg}` body) so the mapping is
//! consistent across seams.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

#[derive(Debug, thiserror::Error)]
pub enum CatalogEngineError {
    /// The addressed namespace/table/view/warehouse does not exist → 404.
    #[error("not found: {0}")]
    NotFound(String),
    /// The request itself is malformed (bad ident, invalid policy) → 400.
    #[error("bad request: {0}")]
    BadRequest(String),
    /// A state conflict (e.g. already exists / protected) → 409.
    #[error("conflict: {0}")]
    Conflict(String),
    /// A lakekeeper / remote-storage call failed with no clean classification
    /// → 502 (bad gateway): the request was well-formed but the upstream engine
    /// could not satisfy it.
    #[error("upstream error: {0}")]
    Upstream(String),
    /// Anything else — preserves today's opaque-500 behavior for unclassifiable
    /// errors (see `From<anyhow::Error>`).
    #[error("internal error: {0}")]
    Internal(String),
}

/// Safe fallback: an untyped `anyhow` error that bubbled up from lakekeeper (or
/// elsewhere) with no clean classification maps to `Internal` — this PRESERVES
/// the pre-E3d-2 500 behavior for cases we cannot justify reclassifying.
impl From<anyhow::Error> for CatalogEngineError {
    fn from(e: anyhow::Error) -> Self {
        CatalogEngineError::Internal(e.to_string())
    }
}

impl CatalogEngineError {
    /// Classify a lakekeeper [`IcebergErrorResponse`](lakekeeper::api::IcebergErrorResponse)
    /// by the HTTP status **lakekeeper itself assigned** (`error.code`), prefixing
    /// `context` for debuggability. This is a faithful passthrough of lakekeeper's
    /// own status — not an invented classification — so a "namespace not found"
    /// (404) or an invalid request (400)/conflict (409) surfaces with the right
    /// code, while 5xx (and anything unrecognized) degrades to `Upstream` (502):
    /// the embedded engine could not satisfy a well-formed request.
    pub fn from_lakekeeper(context: &str, e: lakekeeper::api::IcebergErrorResponse) -> Self {
        let msg = format!("{context}: {}", e.error.message);
        match e.error.code {
            404 => CatalogEngineError::NotFound(msg),
            409 => CatalogEngineError::Conflict(msg),
            400..=499 => CatalogEngineError::BadRequest(msg),
            _ => CatalogEngineError::Upstream(msg),
        }
    }
}

impl IntoResponse for CatalogEngineError {
    fn into_response(self) -> Response {
        let (status, msg) = match &self {
            CatalogEngineError::NotFound(_) => (StatusCode::NOT_FOUND, self.to_string()),
            CatalogEngineError::BadRequest(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            CatalogEngineError::Conflict(_) => (StatusCode::CONFLICT, self.to_string()),
            CatalogEngineError::Upstream(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
            CatalogEngineError::Internal(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            ),
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::response::IntoResponse;

    /// Collect the JSON body of a response into `{"error": ...}`.
    async fn body_json(resp: Response) -> serde_json::Value {
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn not_found_maps_to_404() {
        let resp = CatalogEngineError::NotFound("ns".into()).into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        // Lock down the `{"error": msg}` body shape (message passed through).
        assert_eq!(
            body_json(resp).await,
            serde_json::json!({"error": "not found: ns"})
        );
    }

    #[test]
    fn bad_request_maps_to_400() {
        let resp = CatalogEngineError::BadRequest("bad".into()).into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn conflict_maps_to_409() {
        let resp = CatalogEngineError::Conflict("dup".into()).into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn upstream_maps_to_502() {
        let resp = CatalogEngineError::Upstream("lakekeeper".into()).into_response();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn internal_maps_to_500_and_redacts_detail() {
        let resp = CatalogEngineError::Internal("secret db dsn leaked here".into()).into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        // The sensitive inner detail MUST NOT leak: the body is redacted to the
        // generic "internal error".
        assert_eq!(
            body_json(resp).await,
            serde_json::json!({"error": "internal error"})
        );
    }
}
