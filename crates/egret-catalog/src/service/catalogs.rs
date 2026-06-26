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

    /// Resolve a workspace's catalog by the workspace slug (REST {prefix}).
    pub async fn get_catalog_by_slug(&self, slug: &str) -> Result<Catalog, CatalogError> {
        sqlx::query_as::<_, Catalog>(
            "SELECT c.id, c.workspace_id, c.name, c.warehouse_location \
             FROM catalogs c JOIN workspaces w ON w.id = c.workspace_id WHERE w.slug = $1",
        )
        .bind(slug)
        .fetch_optional(self.pool())
        .await
        .map_err(CatalogError::Db)?
        .ok_or_else(|| CatalogError::NotFound(format!("catalog for workspace slug {slug}")))
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
