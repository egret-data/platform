//! Per-request catalog resolution from the Iceberg REST `{prefix}` path segment.
//!
//! Every prefixed route resolves the workspace's managed catalog from `{prefix}`
//! (the workspace slug) instead of a process-global catalog id. A missing slug
//! maps to `CatalogError::NotFound` → 404.

use crate::routes::CatalogState;
use crate::CatalogError;
use uuid::Uuid;

/// The catalog a request targets, resolved from its `{prefix}`.
pub struct ResolvedCatalog {
    pub catalog_id: Uuid,
    pub warehouse: String,
}

/// Resolve the target catalog for a request from its `{prefix}` (workspace slug).
pub async fn resolve_catalog(
    s: &CatalogState,
    prefix: &str,
) -> Result<ResolvedCatalog, CatalogError> {
    let c = s.service.store.get_catalog_by_slug(prefix).await?;
    Ok(ResolvedCatalog {
        catalog_id: c.id,
        warehouse: c.warehouse_location,
    })
}
