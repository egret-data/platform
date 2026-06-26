//! Per-request catalog resolution from the Iceberg REST `{prefix}` path segment.
//!
//! Every prefixed route resolves the workspace's managed catalog from `{prefix}`
//! (the workspace slug) instead of a process-global catalog id. A missing slug
//! maps to `CatalogError::NotFound` → 404.

use crate::routes::CatalogState;
use crate::CatalogError;
use uuid::Uuid;

/// The authenticated identity behind a request, established by `require_bearer`
/// and threaded into handlers via `axum::Extension`.
#[derive(Clone, Debug)]
pub enum AuthnPrincipal {
    /// The static config token: an ops/test/discovery override that bypasses
    /// per-workspace ownership checks.
    Admin,
    /// A per-workspace catalog token; may only touch its own workspace's catalog.
    Workspace(Uuid),
}

/// The catalog a request targets, resolved from its `{prefix}`.
pub struct ResolvedCatalog {
    pub catalog_id: Uuid,
    pub warehouse: String,
}

/// Resolve the target catalog for a request from its `{prefix}` (workspace slug)
/// and enforce that `principal` is allowed to touch it.
///
/// - Unknown `{prefix}` → `NotFound` (404).
/// - Authenticated workspace token on a *foreign* workspace's catalog →
///   `Forbidden` (403).
/// - `Admin` (static config token) bypasses the ownership check.
pub async fn resolve_catalog(
    s: &CatalogState,
    principal: &AuthnPrincipal,
    prefix: &str,
) -> Result<ResolvedCatalog, CatalogError> {
    let c = s.service.store.get_catalog_by_slug(prefix).await?;
    match principal {
        AuthnPrincipal::Admin => {}
        AuthnPrincipal::Workspace(ws) if *ws == c.workspace_id => {}
        AuthnPrincipal::Workspace(_) => {
            return Err(CatalogError::Forbidden(format!(
                "token is not authorized for workspace prefix `{prefix}`"
            )));
        }
    }
    Ok(ResolvedCatalog {
        catalog_id: c.id,
        warehouse: c.warehouse_location,
    })
}
