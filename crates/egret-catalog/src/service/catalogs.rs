//! `catalogs` table: one managed Iceberg catalog per workspace.
use crate::service::CatalogStore;
use crate::CatalogError;
use uuid::Uuid;

/// A workspace's managed catalog row.
///
/// NOTE: the `catalogs` table also has `properties jsonb` and `created_at`,
/// intentionally omitted here (read-only MVP). If you add a field, you MUST also
/// add the column to BOTH the SELECT and RETURNING lists — `sqlx::FromRow` matches
/// by name and will error at runtime on a missing column.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Catalog {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub warehouse_location: String,
}

impl CatalogStore {
    /// Create the managed catalog for a workspace (one per workspace).
    pub async fn create_catalog(
        &self,
        workspace_id: Uuid,
        name: &str,
        warehouse_location: &str,
    ) -> Result<Catalog, CatalogError> {
        sqlx::query_as::<_, Catalog>(
            "INSERT INTO catalogs (workspace_id, name, warehouse_location) \
             VALUES ($1, $2, $3) RETURNING id, workspace_id, name, warehouse_location",
        )
        .bind(workspace_id)
        .bind(name)
        .bind(warehouse_location)
        .fetch_one(self.pool())
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.is_unique_violation() => {
                CatalogError::AlreadyExists(format!("catalog for workspace {workspace_id}"))
            }
            _ => CatalogError::Db(e),
        })
    }

    /// Load a workspace's catalog, or NotFound.
    pub async fn get_catalog_by_workspace(
        &self,
        workspace_id: Uuid,
    ) -> Result<Catalog, CatalogError> {
        sqlx::query_as::<_, Catalog>(
            "SELECT id, workspace_id, name, warehouse_location \
             FROM catalogs WHERE workspace_id = $1",
        )
        .bind(workspace_id)
        .fetch_optional(self.pool())
        .await
        .map_err(CatalogError::Db)?
        .ok_or_else(|| CatalogError::NotFound(format!("catalog for workspace {workspace_id}")))
    }

    /// Ensure a single default catalog exists and return its id (idempotent).
    ///
    /// TEMP (Plan A-1): picks the first workspace and lazily creates a `default`
    /// catalog for it. Replaced by per-request `{prefix}` resolution in Plan A-2.
    pub async fn ensure_default_catalog(&self, warehouse: &str) -> Result<Uuid, CatalogError> {
        let workspace_id: Uuid =
            sqlx::query_scalar::<_, Uuid>("SELECT id FROM workspaces ORDER BY created_at LIMIT 1")
                .fetch_optional(self.pool())
                .await
                .map_err(CatalogError::Db)?
                .ok_or_else(|| {
                    CatalogError::NotFound(
                        "no workspace exists to bootstrap a default catalog".into(),
                    )
                })?;
        match self.get_catalog_by_workspace(workspace_id).await {
            Ok(catalog) => Ok(catalog.id),
            Err(CatalogError::NotFound(_)) => Ok(self
                .create_catalog(workspace_id, "default", warehouse)
                .await?
                .id),
            Err(e) => Err(e),
        }
    }

    /// Load a catalog by its id, or NotFound.
    pub async fn get_catalog_by_id(&self, id: Uuid) -> Result<Catalog, CatalogError> {
        sqlx::query_as::<_, Catalog>(
            "SELECT id, workspace_id, name, warehouse_location \
             FROM catalogs WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await
        .map_err(CatalogError::Db)?
        .ok_or_else(|| CatalogError::NotFound(format!("catalog {id}")))
    }
}
