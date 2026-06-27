//! workspace → warehouse UUID mapping store.
//!
//! Uses the egret-side sqlx-0.8 pool (via `CatalogStore::pool()`). All queries
//! target the `workspace_warehouse` table created in migration 0025.

use crate::CatalogError;
use uuid::Uuid;

use super::store::CatalogStore;

impl CatalogStore {
    /// Persist a workspace → warehouse mapping. Idempotent: if a row already
    /// exists for `workspace_id` the INSERT is silently ignored (first-write-wins).
    pub async fn upsert_warehouse(
        &self,
        workspace_id: Uuid,
        warehouse_id: Uuid,
    ) -> Result<(), CatalogError> {
        sqlx::query(
            "INSERT INTO workspace_warehouse (workspace_id, warehouse_id) \
             VALUES ($1, $2) \
             ON CONFLICT (workspace_id) DO NOTHING",
        )
        .bind(workspace_id)
        .bind(warehouse_id)
        .execute(self.pool())
        .await
        .map_err(CatalogError::Db)?;
        Ok(())
    }

    /// Return the lakekeeper warehouse UUID for `workspace_id`, or `None` if
    /// no warehouse has been provisioned yet.
    pub async fn warehouse_for_workspace(
        &self,
        workspace_id: Uuid,
    ) -> Result<Option<Uuid>, CatalogError> {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT warehouse_id FROM workspace_warehouse WHERE workspace_id = $1",
        )
        .bind(workspace_id)
        .fetch_optional(self.pool())
        .await
        .map_err(CatalogError::Db)
    }

    /// Return the lakekeeper warehouse UUID for the workspace identified by
    /// `slug`, or `None` if the slug is unknown or has no warehouse yet.
    pub async fn warehouse_for_slug(&self, slug: &str) -> Result<Option<Uuid>, CatalogError> {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT ww.warehouse_id \
             FROM workspace_warehouse ww \
             JOIN workspaces w ON w.id = ww.workspace_id \
             WHERE w.slug = $1",
        )
        .bind(slug)
        .fetch_optional(self.pool())
        .await
        .map_err(CatalogError::Db)
    }
}
