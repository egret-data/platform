//! Per-workspace warehouse store.
//!
//! Uses the egret-side sqlx-0.8 pool (via `CatalogStore::pool()`). All queries
//! target the `workspace_warehouses` table created in migration 0034, which
//! replaces the legacy 1:1 `workspace_warehouse` table (dropped in 0034).
//!
//! Each workspace may now own N warehouses, each optionally backed by a
//! registered storage connection (`workspace_storages.id`). Every method is
//! workspace-scoped: a warehouse row owned by a different workspace is treated
//! as absent (`CatalogError::NotFound`), closing cross-tenant IDORs.

use crate::CatalogError;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::store::CatalogStore;

/// A user-managed warehouse row as returned from the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceWarehouse {
    /// egret-side primary key for this warehouse row.
    pub id: Uuid,
    /// The lakekeeper warehouse UUID this row maps to.
    pub warehouse_id: Uuid,
    /// Human-facing name, unique within the workspace.
    pub name: String,
    /// Optional registered storage connection backing this warehouse.
    pub storage_conn_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

/// Internal sqlx row type for `workspace_warehouses` SELECTs.
#[derive(sqlx::FromRow)]
struct WarehouseRow {
    id: Uuid,
    warehouse_id: Uuid,
    name: String,
    storage_conn_id: Option<Uuid>,
    created_at: DateTime<Utc>,
}

impl From<WarehouseRow> for WorkspaceWarehouse {
    fn from(r: WarehouseRow) -> Self {
        WorkspaceWarehouse {
            id: r.id,
            warehouse_id: r.warehouse_id,
            name: r.name,
            storage_conn_id: r.storage_conn_id,
            created_at: r.created_at,
        }
    }
}

/// Map a sqlx error to `AlreadyExists(name)` on a unique-constraint violation,
/// otherwise pass it through as `Db`.
fn map_unique_violation(e: sqlx::Error, name: &str) -> CatalogError {
    match &e {
        sqlx::Error::Database(d) if d.is_unique_violation() => {
            CatalogError::AlreadyExists(name.to_string())
        }
        _ => CatalogError::Db(e),
    }
}

impl CatalogStore {
    // -----------------------------------------------------------------------
    // Per-warehouse CRUD (migration 0034)
    // -----------------------------------------------------------------------

    /// Insert a new warehouse row for `workspace_id`.
    ///
    /// `warehouse_id` is the lakekeeper warehouse UUID; `name` must be unique
    /// within the workspace (a duplicate maps to `CatalogError::AlreadyExists`).
    pub async fn insert_warehouse(
        &self,
        workspace_id: Uuid,
        warehouse_id: Uuid,
        name: &str,
        storage_conn_id: Option<Uuid>,
    ) -> Result<WorkspaceWarehouse, CatalogError> {
        let row: WarehouseRow = sqlx::query_as::<_, WarehouseRow>(
            "INSERT INTO workspace_warehouses \
             (workspace_id, warehouse_id, name, storage_conn_id) \
             VALUES ($1, $2, $3, $4) \
             RETURNING id, warehouse_id, name, storage_conn_id, created_at",
        )
        .bind(workspace_id)
        .bind(warehouse_id)
        .bind(name)
        .bind(storage_conn_id)
        .fetch_one(self.pool())
        .await
        .map_err(|e| map_unique_violation(e, name))?;

        Ok(row.into())
    }

    /// List all warehouses owned by `workspace_id`, ordered by name.
    pub async fn list_warehouses(
        &self,
        workspace_id: Uuid,
    ) -> Result<Vec<WorkspaceWarehouse>, CatalogError> {
        let rows: Vec<WarehouseRow> = sqlx::query_as::<_, WarehouseRow>(
            "SELECT id, warehouse_id, name, storage_conn_id, created_at \
             FROM workspace_warehouses \
             WHERE workspace_id = $1 \
             ORDER BY name ASC",
        )
        .bind(workspace_id)
        .fetch_all(self.pool())
        .await
        .map_err(CatalogError::Db)?;

        Ok(rows.into_iter().map(WorkspaceWarehouse::from).collect())
    }

    /// Fetch a single warehouse by its egret `id`, scoped to `workspace_id`.
    ///
    /// A row owned by another workspace (or a missing id) maps to
    /// `CatalogError::NotFound`.
    pub async fn get_warehouse(
        &self,
        workspace_id: Uuid,
        id: Uuid,
    ) -> Result<WorkspaceWarehouse, CatalogError> {
        let row: WarehouseRow = sqlx::query_as::<_, WarehouseRow>(
            "SELECT id, warehouse_id, name, storage_conn_id, created_at \
             FROM workspace_warehouses \
             WHERE id = $1 AND workspace_id = $2",
        )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(self.pool())
        .await
        .map_err(CatalogError::Db)?
        .ok_or_else(|| CatalogError::NotFound("warehouse".into()))?;

        Ok(row.into())
    }

    /// Rename a warehouse, scoped to `workspace_id`.
    ///
    /// Returns `CatalogError::NotFound` when no matching row exists in the
    /// workspace, or `CatalogError::AlreadyExists` when the new name collides
    /// with another warehouse in the same workspace.
    pub async fn rename_warehouse(
        &self,
        workspace_id: Uuid,
        id: Uuid,
        name: &str,
    ) -> Result<(), CatalogError> {
        let r = sqlx::query(
            "UPDATE workspace_warehouses SET name = $1 \
             WHERE id = $2 AND workspace_id = $3",
        )
        .bind(name)
        .bind(id)
        .bind(workspace_id)
        .execute(self.pool())
        .await
        .map_err(|e| map_unique_violation(e, name))?;

        if r.rows_affected() == 0 {
            return Err(CatalogError::NotFound("warehouse".into()));
        }
        Ok(())
    }

    /// Delete a warehouse row, scoped to `workspace_id`, returning the
    /// lakekeeper `warehouse_id` so the caller can drive the lakekeeper-side
    /// delete afterwards.
    ///
    /// Returns `CatalogError::NotFound` when no matching row exists in the
    /// workspace.
    pub async fn delete_warehouse_row(
        &self,
        workspace_id: Uuid,
        id: Uuid,
    ) -> Result<Uuid, CatalogError> {
        // DELETE ... RETURNING is atomic and ownership-scoped in one round trip;
        // an absent / cross-workspace row yields no returned value → NotFound.
        let lakekeeper_id: Uuid = sqlx::query_scalar::<_, Uuid>(
            "DELETE FROM workspace_warehouses \
             WHERE id = $1 AND workspace_id = $2 \
             RETURNING warehouse_id",
        )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(self.pool())
        .await
        .map_err(CatalogError::Db)?
        .ok_or_else(|| CatalogError::NotFound("warehouse".into()))?;

        Ok(lakekeeper_id)
    }

    /// Resolve an egret warehouse `id` to its lakekeeper `warehouse_id`,
    /// scoped to `workspace_id`.
    ///
    /// Returns `CatalogError::NotFound` for a missing or cross-workspace id.
    pub async fn lakekeeper_id(&self, workspace_id: Uuid, id: Uuid) -> Result<Uuid, CatalogError> {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT warehouse_id FROM workspace_warehouses \
             WHERE id = $1 AND workspace_id = $2",
        )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(self.pool())
        .await
        .map_err(CatalogError::Db)?
        .ok_or_else(|| CatalogError::NotFound("warehouse".into()))
    }

    // -----------------------------------------------------------------------
    // Legacy 1:1 helpers — KEPT so the engine / front_auth crates keep
    // compiling until the next task migrates their callers. Repointed at the
    // new `workspace_warehouses` table (the legacy `workspace_warehouse` table
    // is dropped in migration 0034). Each picks the `name = 'default'` row,
    // falling back to the earliest-created warehouse for the workspace.
    // -----------------------------------------------------------------------

    /// Persist a workspace → warehouse mapping (legacy). Idempotent: if a
    /// `'default'` warehouse already exists for `workspace_id` the INSERT is
    /// silently ignored (first-write-wins on the `(workspace_id, name)` unique).
    pub async fn upsert_warehouse(
        &self,
        workspace_id: Uuid,
        warehouse_id: Uuid,
    ) -> Result<(), CatalogError> {
        sqlx::query(
            "INSERT INTO workspace_warehouses (workspace_id, warehouse_id, name) \
             VALUES ($1, $2, 'default') \
             ON CONFLICT (workspace_id, name) DO NOTHING",
        )
        .bind(workspace_id)
        .bind(warehouse_id)
        .execute(self.pool())
        .await
        .map_err(CatalogError::Db)?;
        Ok(())
    }

    /// Return the lakekeeper warehouse UUID for `workspace_id` (legacy), or
    /// `None` if no warehouse has been provisioned yet. Prefers the
    /// `'default'` warehouse, then the earliest-created one.
    pub async fn warehouse_for_workspace(
        &self,
        workspace_id: Uuid,
    ) -> Result<Option<Uuid>, CatalogError> {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT warehouse_id FROM workspace_warehouses \
             WHERE workspace_id = $1 \
             ORDER BY (name = 'default') DESC, created_at ASC \
             LIMIT 1",
        )
        .bind(workspace_id)
        .fetch_optional(self.pool())
        .await
        .map_err(CatalogError::Db)
    }

    /// Return the lakekeeper warehouse UUID for the workspace identified by
    /// `slug` (legacy), or `None` if the slug is unknown or has no warehouse
    /// yet. Prefers the `'default'` warehouse, then the earliest-created one.
    pub async fn warehouse_for_slug(&self, slug: &str) -> Result<Option<Uuid>, CatalogError> {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT ww.warehouse_id \
             FROM workspace_warehouses ww \
             JOIN workspaces w ON w.id = ww.workspace_id \
             WHERE w.slug = $1 \
             ORDER BY (ww.name = 'default') DESC, ww.created_at ASC \
             LIMIT 1",
        )
        .bind(slug)
        .fetch_optional(self.pool())
        .await
        .map_err(CatalogError::Db)
    }
}
