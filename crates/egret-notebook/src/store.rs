//! `NotebookStore` — parameterized Postgres persistence for notebook documents
//! and their ordered cells.

use egret_metastore::Db;
use uuid::Uuid;

use crate::error::NotebookApiError;
use crate::model::{
    CatalogOption, Cell, EngineOption, NotebookDetail, NotebookSettings, NotebookSummary,
    UpdateCell, UpdateNotebook,
};

// ── Row structs ──────────────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct NotebookRow {
    id: Uuid,
    workspace_id: Uuid,
    owner_user_id: Uuid,
    name: String,
    description: String,
    is_public: bool,
    updated_at: chrono::DateTime<chrono::Utc>,
    folder_id: Option<Uuid>,
    deleted_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl NotebookRow {
    fn into_summary(self, viewer: Uuid, bookmarked: bool) -> NotebookSummary {
        let is_owner = self.owner_user_id == viewer;
        NotebookSummary {
            id: self.id,
            name: self.name,
            description: self.description,
            is_public: self.is_public,
            owner_user_id: self.owner_user_id,
            is_owner,
            updated_at: self.updated_at,
            bookmarked,
            folder_id: self.folder_id,
            deleted_at: self.deleted_at,
        }
    }

    /// Convert without a viewer (ownership unknown at the store level).
    /// `bookmarked` is left false; the detail page doesn't surface it.
    fn into_summary_unknown_owner(self) -> (NotebookSummary, Uuid) {
        let ws = self.workspace_id;
        let owner = self.owner_user_id;
        let summary = NotebookSummary {
            id: self.id,
            name: self.name,
            description: self.description,
            is_public: self.is_public,
            owner_user_id: owner,
            // caller (route) sets this
            is_owner: false,
            updated_at: self.updated_at,
            bookmarked: false,
            folder_id: self.folder_id,
            deleted_at: self.deleted_at,
        };
        (summary, ws)
    }
}

/// Row for the list query: a notebook plus the per-viewer `bookmarked` flag.
#[derive(sqlx::FromRow)]
struct NotebookListRow {
    #[sqlx(flatten)]
    nb: NotebookRow,
    bookmarked: bool,
}

#[derive(sqlx::FromRow)]
struct CellRow {
    id: Uuid,
    cell_type: String,
    source: String,
    cell_order: i32,
    execution_count: Option<i32>,
    last_output: Option<serde_json::Value>,
    last_execution_time_ms: Option<i32>,
}

impl CellRow {
    fn into_cell(self) -> Cell {
        Cell {
            id: self.id,
            cell_type: self.cell_type,
            source: self.source,
            cell_order: self.cell_order,
            execution_count: self.execution_count,
            last_output: self.last_output,
            last_execution_time_ms: self.last_execution_time_ms,
        }
    }
}

// ── Store ────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct NotebookStore {
    db: Db,
}

const NB_COLS: &str = "id, workspace_id, owner_user_id, name, description, is_public, \
     updated_at, folder_id, deleted_at";

impl NotebookStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Create a notebook with one empty `code` cell, atomically.
    pub async fn create(
        &self,
        ws: Uuid,
        owner: Uuid,
        name: &str,
    ) -> Result<Uuid, NotebookApiError> {
        let mut tx = self.db.pool.begin().await?;
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO notebooks (workspace_id, owner_user_id, name) \
             VALUES ($1,$2,$3) RETURNING id",
        )
        .bind(ws)
        .bind(owner)
        .bind(name)
        .fetch_one(&mut *tx)
        .await?;

        // Seed one empty code cell (spec requirement).
        sqlx::query(
            "INSERT INTO notebook_cells (notebook_id, cell_type, source, cell_order) \
             VALUES ($1,'code','',0)",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(id)
    }

    /// List notebooks the viewer can see: owned or public, in this workspace.
    /// `trashed` selects the trash bin (soft-deleted) instead of the live list.
    /// Each row carries a per-viewer `bookmarked` flag.
    pub async fn list_visible(
        &self,
        ws: Uuid,
        viewer: Uuid,
        trashed: bool,
    ) -> Result<Vec<NotebookSummary>, NotebookApiError> {
        let pred = if trashed {
            "deleted_at IS NOT NULL"
        } else {
            "deleted_at IS NULL"
        };
        let rows = sqlx::query_as::<_, NotebookListRow>(&format!(
            "SELECT {NB_COLS}, \
             EXISTS (SELECT 1 FROM bookmarks b WHERE b.user_id=$2 \
                 AND b.entity_type='notebook' AND b.entity_id=notebooks.id) AS bookmarked \
             FROM notebooks \
             WHERE workspace_id=$1 AND (owner_user_id=$2 OR is_public) AND {pred} \
             ORDER BY updated_at DESC"
        ))
        .bind(ws)
        .bind(viewer)
        .fetch_all(&self.db.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| r.nb.into_summary(viewer, r.bookmarked))
            .collect())
    }

    /// Fetch a notebook without workspace/ownership scoping (route enforces that).
    /// Returns `(summary, workspace_id)` so the route can verify workspace match.
    pub async fn get_summary(
        &self,
        id: Uuid,
    ) -> Result<Option<(NotebookSummary, Uuid)>, NotebookApiError> {
        let row = sqlx::query_as::<_, NotebookRow>(&format!(
            "SELECT {NB_COLS} FROM notebooks WHERE id=$1"
        ))
        .bind(id)
        .fetch_optional(&self.db.pool)
        .await?;
        Ok(row.map(NotebookRow::into_summary_unknown_owner))
    }

    /// Fetch a notebook and all its cells (cells ordered by cell_order, created_at).
    /// `viewer` is used to set `summary.is_owner` so callers can't forget it.
    pub async fn get_with_cells(
        &self,
        id: Uuid,
        viewer: Uuid,
    ) -> Result<Option<NotebookDetail>, NotebookApiError> {
        let Some((mut summary, _ws)) = self.get_summary(id).await? else {
            return Ok(None);
        };
        summary.is_owner = summary.owner_user_id == viewer;
        let cells = sqlx::query_as::<_, CellRow>(
            "SELECT id, cell_type, source, cell_order, execution_count, \
             last_output, last_execution_time_ms \
             FROM notebook_cells \
             WHERE notebook_id=$1 \
             ORDER BY cell_order ASC, created_at ASC",
        )
        .bind(id)
        .fetch_all(&self.db.pool)
        .await?;
        Ok(Some(NotebookDetail {
            summary,
            cells: cells.into_iter().map(CellRow::into_cell).collect(),
        }))
    }

    /// Partial update: only non-None fields are changed. Sets `updated_at=now()`.
    pub async fn update(&self, id: Uuid, p: &UpdateNotebook) -> Result<(), NotebookApiError> {
        sqlx::query(
            "UPDATE notebooks SET \
             name        = COALESCE($2, name), \
             description = COALESCE($3, description), \
             is_public   = COALESCE($4, is_public), \
             updated_at  = now() \
             WHERE id=$1",
        )
        .bind(id)
        .bind(p.name.as_deref())
        .bind(p.description.as_deref())
        .bind(p.is_public)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// Permanently delete a notebook (cascades to cells via FK). Polymorphic
    /// bookmark rows have no FK on entity_id, so purge them in the same tx.
    pub async fn delete(&self, id: Uuid) -> Result<(), NotebookApiError> {
        let mut tx = self.db.pool.begin().await?;
        sqlx::query("DELETE FROM bookmarks WHERE entity_type='notebook' AND entity_id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM notebooks WHERE id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Soft-delete: move the notebook to the trash bin.
    pub async fn trash(&self, id: Uuid) -> Result<(), NotebookApiError> {
        sqlx::query("UPDATE notebooks SET deleted_at=now(), updated_at=now() WHERE id=$1")
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    /// Restore a soft-deleted notebook from the trash bin.
    pub async fn restore(&self, id: Uuid) -> Result<(), NotebookApiError> {
        sqlx::query("UPDATE notebooks SET deleted_at=NULL, updated_at=now() WHERE id=$1")
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    /// Move a notebook into a folder (or back to Uncategorized with `None`).
    pub async fn set_folder(
        &self,
        id: Uuid,
        folder_id: Option<Uuid>,
    ) -> Result<(), NotebookApiError> {
        sqlx::query("UPDATE notebooks SET folder_id=$2 WHERE id=$1")
            .bind(id)
            .bind(folder_id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    /// `(workspace_id, entity_type)` of a folder, for the set_folder guard.
    /// `None` if the folder doesn't exist.
    pub async fn folder_kind(
        &self,
        folder_id: Uuid,
    ) -> Result<Option<(Uuid, String)>, NotebookApiError> {
        Ok(sqlx::query_as::<_, (Uuid, String)>(
            "SELECT workspace_id, entity_type FROM folders WHERE id=$1",
        )
        .bind(folder_id)
        .fetch_optional(&self.db.pool)
        .await?)
    }

    /// Per-user bookmark on a notebook (idempotent).
    pub async fn add_bookmark(&self, user: Uuid, id: Uuid) -> Result<(), NotebookApiError> {
        sqlx::query(
            "INSERT INTO bookmarks (user_id, entity_type, entity_id) VALUES ($1,'notebook',$2) \
             ON CONFLICT (user_id, entity_type, entity_id) DO NOTHING",
        )
        .bind(user)
        .bind(id)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// Remove a per-user bookmark on a notebook (idempotent).
    pub async fn remove_bookmark(&self, user: Uuid, id: Uuid) -> Result<(), NotebookApiError> {
        sqlx::query(
            "DELETE FROM bookmarks WHERE user_id=$1 AND entity_type='notebook' AND entity_id=$2",
        )
        .bind(user)
        .bind(id)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// Add a cell and bump the notebook's `updated_at`.
    pub async fn add_cell(
        &self,
        nb: Uuid,
        ty: &str,
        source: &str,
        order: i32,
    ) -> Result<Uuid, NotebookApiError> {
        let id = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO notebook_cells (notebook_id, cell_type, source, cell_order) \
             VALUES ($1,$2,$3,$4) RETURNING id",
        )
        .bind(nb)
        .bind(ty)
        .bind(source)
        .bind(order)
        .fetch_one(&self.db.pool)
        .await?;
        self.touch(nb).await?;
        Ok(id)
    }

    /// Partial update on a single cell. Includes `AND notebook_id=$2` to defend
    /// against cross-notebook IDOR at the SQL level; a row that doesn't belong to
    /// `nb` matches nothing → `NotFound` (so the route surfaces a 404).
    pub async fn update_cell(
        &self,
        nb: Uuid,
        cid: Uuid,
        p: &UpdateCell,
    ) -> Result<(), NotebookApiError> {
        let result = sqlx::query(
            "UPDATE notebook_cells SET \
             cell_type  = COALESCE($3, cell_type), \
             source     = COALESCE($4, source), \
             updated_at = now() \
             WHERE id=$1 AND notebook_id=$2",
        )
        .bind(cid)
        .bind(nb)
        .bind(p.cell_type.as_deref())
        .bind(p.source.as_deref())
        .execute(&self.db.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(NotebookApiError::from(egret_core::Error::NotFound(
                "cell".into(),
            )));
        }
        self.touch(nb).await?;
        Ok(())
    }

    /// Persist a cell's captured outputs (capped), execution count, and elapsed ms.
    /// Called by the E-P2 recorder; `AND notebook_id=$2` guards cross-notebook writes.
    pub async fn record_output(
        &self,
        nb: Uuid,
        cid: Uuid,
        outputs: &[crate::output::CellOutput],
        execution_count: Option<i32>,
        elapsed_ms: Option<i32>,
    ) -> Result<(), NotebookApiError> {
        let capped = crate::output::cap_outputs(outputs.to_vec());
        let json = serde_json::to_value(&capped)
            .map_err(|e| NotebookApiError::from(egret_core::Error::Internal(anyhow::anyhow!(e))))?;
        sqlx::query(
            "UPDATE notebook_cells SET \
             last_output = $3, execution_count = $4, last_execution_time_ms = $5, \
             updated_at = now() \
             WHERE id=$1 AND notebook_id=$2",
        )
        .bind(cid)
        .bind(nb)
        .bind(json)
        .bind(execution_count)
        .bind(elapsed_ms)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// Delete a cell. Includes `AND notebook_id=$2` (cross-notebook IDOR defense);
    /// a row that doesn't belong to `nb` matches nothing → `NotFound` (404).
    pub async fn delete_cell(&self, nb: Uuid, cid: Uuid) -> Result<(), NotebookApiError> {
        let result = sqlx::query("DELETE FROM notebook_cells WHERE id=$1 AND notebook_id=$2")
            .bind(cid)
            .bind(nb)
            .execute(&self.db.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(NotebookApiError::from(egret_core::Error::NotFound(
                "cell".into(),
            )));
        }
        self.touch(nb).await?;
        Ok(())
    }

    /// Reorder cells: assign each UUID in `order` the position it occupies.
    /// Includes `AND notebook_id=$3` per-UPDATE (cross-notebook IDOR defense).
    pub async fn reorder(&self, nb: Uuid, order: &[Uuid]) -> Result<(), NotebookApiError> {
        let mut tx = self.db.pool.begin().await?;
        for (i, cid) in order.iter().enumerate() {
            sqlx::query(
                "UPDATE notebook_cells SET cell_order=$1, updated_at=now() \
                 WHERE id=$2 AND notebook_id=$3",
            )
            .bind(i as i32)
            .bind(cid)
            .bind(nb)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        self.touch(nb).await?;
        Ok(())
    }

    async fn touch(&self, nb: Uuid) -> Result<(), NotebookApiError> {
        sqlx::query("UPDATE notebooks SET updated_at=now() WHERE id=$1")
            .bind(nb)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    // ── Notebook settings: engine + catalog access ───────────────────────────

    /// List all engine (DB connection) options for this workspace with enabled flag.
    pub async fn list_engine_options(
        &self,
        ws: Uuid,
    ) -> Result<Vec<EngineOption>, NotebookApiError> {
        #[derive(sqlx::FromRow)]
        struct Row {
            id: Uuid,
            name: String,
            engine: String,
            enabled: bool,
        }
        let rows = sqlx::query_as::<_, Row>(
            "SELECT d.id, d.name, d.engine, \
             (nea.database_id IS NOT NULL) AS enabled \
             FROM databases d \
             LEFT JOIN notebook_engine_access nea \
                 ON nea.database_id = d.id AND nea.workspace_id = $1 \
             WHERE d.workspace_id = $1 \
             ORDER BY d.name",
        )
        .bind(ws)
        .fetch_all(&self.db.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| EngineOption {
                id: r.id,
                name: r.name,
                engine: r.engine,
                enabled: r.enabled,
            })
            .collect())
    }

    /// List all catalog (workspace warehouse) options for this workspace with enabled flag.
    pub async fn list_catalog_options(
        &self,
        ws: Uuid,
    ) -> Result<Vec<CatalogOption>, NotebookApiError> {
        #[derive(sqlx::FromRow)]
        struct Row {
            id: Uuid,
            name: String,
            enabled: bool,
        }
        let rows = sqlx::query_as::<_, Row>(
            "SELECT ww.id, ww.name, \
             (nca.warehouse_id IS NOT NULL) AS enabled \
             FROM workspace_warehouses ww \
             LEFT JOIN notebook_catalog_access nca \
                 ON nca.warehouse_id = ww.id AND nca.workspace_id = $1 \
             WHERE ww.workspace_id = $1 \
             ORDER BY ww.name",
        )
        .bind(ws)
        .fetch_all(&self.db.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| CatalogOption {
                id: r.id,
                name: r.name,
                enabled: r.enabled,
            })
            .collect())
    }

    /// Get current notebook settings (engines + catalogs with enabled flags).
    pub async fn get_settings(&self, ws: Uuid) -> Result<NotebookSettings, NotebookApiError> {
        let engines = self.list_engine_options(ws).await?;
        let catalogs = self.list_catalog_options(ws).await?;
        Ok(NotebookSettings { engines, catalogs })
    }

    /// Replace the engine access set for a workspace.
    ///
    /// Validates that every id in `engine_ids` belongs to this workspace
    /// (returns `InvalidInput` if any id is foreign/absent — no cross-tenant leakage).
    /// Then atomically: DELETE existing rows for this workspace + INSERT the new set.
    pub async fn set_engine_access(
        &self,
        ws: Uuid,
        engine_ids: &[Uuid],
    ) -> Result<(), NotebookApiError> {
        if !engine_ids.is_empty() {
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM databases WHERE id = ANY($1) AND workspace_id = $2",
            )
            .bind(engine_ids)
            .bind(ws)
            .fetch_one(&self.db.pool)
            .await?;
            if count as usize != engine_ids.len() {
                return Err(egret_core::Error::InvalidInput(
                    "one or more engine_ids do not belong to this workspace".into(),
                )
                .into());
            }
        }
        let mut tx = self.db.pool.begin().await?;
        sqlx::query("DELETE FROM notebook_engine_access WHERE workspace_id = $1")
            .bind(ws)
            .execute(&mut *tx)
            .await?;
        for &db_id in engine_ids {
            sqlx::query(
                "INSERT INTO notebook_engine_access (workspace_id, database_id) VALUES ($1, $2)",
            )
            .bind(ws)
            .bind(db_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Replace the catalog (warehouse) access set for a workspace.
    ///
    /// Validates that every id in `catalog_ids` belongs to this workspace.
    /// The `catalog_ids` in the API refer to `workspace_warehouses.id` rows.
    pub async fn set_catalog_access(
        &self,
        ws: Uuid,
        catalog_ids: &[Uuid],
    ) -> Result<(), NotebookApiError> {
        if !catalog_ids.is_empty() {
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM workspace_warehouses WHERE id = ANY($1) AND workspace_id = $2",
            )
            .bind(catalog_ids)
            .bind(ws)
            .fetch_one(&self.db.pool)
            .await?;
            if count as usize != catalog_ids.len() {
                return Err(egret_core::Error::InvalidInput(
                    "one or more catalog_ids do not belong to this workspace".into(),
                )
                .into());
            }
        }
        let mut tx = self.db.pool.begin().await?;
        sqlx::query("DELETE FROM notebook_catalog_access WHERE workspace_id = $1")
            .bind(ws)
            .execute(&mut *tx)
            .await?;
        for &wh_id in catalog_ids {
            sqlx::query(
                "INSERT INTO notebook_catalog_access (workspace_id, warehouse_id) VALUES ($1, $2)",
            )
            .bind(ws)
            .bind(wh_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Replace BOTH the engine and catalog access sets for a workspace atomically.
    ///
    /// All ids are validated up front (engines against `databases`, catalogs
    /// against `workspace_warehouses`) BEFORE any mutation: if either set
    /// contains an id that does not belong to `ws`, this returns `InvalidInput`
    /// (→ 400) and nothing is changed. On success, a single transaction replaces
    /// both sets (DELETE+INSERT each), so a rejected PUT is a guaranteed no-op
    /// and the two sets can never be left half-applied.
    pub async fn set_notebook_access(
        &self,
        ws: Uuid,
        engine_ids: &[Uuid],
        catalog_ids: &[Uuid],
    ) -> Result<(), NotebookApiError> {
        // ── Validate everything before mutating anything ─────────────────────
        if !engine_ids.is_empty() {
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM databases WHERE id = ANY($1) AND workspace_id = $2",
            )
            .bind(engine_ids)
            .bind(ws)
            .fetch_one(&self.db.pool)
            .await?;
            if count as usize != engine_ids.len() {
                return Err(egret_core::Error::InvalidInput(
                    "one or more engine_ids do not belong to this workspace".into(),
                )
                .into());
            }
        }
        if !catalog_ids.is_empty() {
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM workspace_warehouses WHERE id = ANY($1) AND workspace_id = $2",
            )
            .bind(catalog_ids)
            .bind(ws)
            .fetch_one(&self.db.pool)
            .await?;
            if count as usize != catalog_ids.len() {
                return Err(egret_core::Error::InvalidInput(
                    "one or more catalog_ids do not belong to this workspace".into(),
                )
                .into());
            }
        }

        // ── One transaction: replace both sets atomically ────────────────────
        let mut tx = self.db.pool.begin().await?;
        sqlx::query("DELETE FROM notebook_engine_access WHERE workspace_id = $1")
            .bind(ws)
            .execute(&mut *tx)
            .await?;
        for &db_id in engine_ids {
            sqlx::query(
                "INSERT INTO notebook_engine_access (workspace_id, database_id) VALUES ($1, $2)",
            )
            .bind(ws)
            .bind(db_id)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query("DELETE FROM notebook_catalog_access WHERE workspace_id = $1")
            .bind(ws)
            .execute(&mut *tx)
            .await?;
        for &wh_id in catalog_ids {
            sqlx::query(
                "INSERT INTO notebook_catalog_access (workspace_id, warehouse_id) VALUES ($1, $2)",
            )
            .bind(ws)
            .bind(wh_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use egret_metastore::Db;

    /// Seed a workspace + primary user. Returns (workspace_id, user_id).
    /// Matches the NOT NULL columns of migrations/0002 (users) and 0003 (workspaces).
    async fn seed(db: &Db) -> (Uuid, Uuid) {
        let user_id: Uuid = sqlx::query_scalar(
            "INSERT INTO users (provider, email, name) VALUES ('local','alice@test.local','Alice') \
             RETURNING id",
        )
        .fetch_one(&db.pool)
        .await
        .expect("insert user");

        let ws_id: Uuid = sqlx::query_scalar(
            "INSERT INTO workspaces (slug, name, created_by) VALUES ('test-ws','Test WS',$1) \
             RETURNING id",
        )
        .bind(user_id)
        .fetch_one(&db.pool)
        .await
        .expect("insert workspace");

        (ws_id, user_id)
    }

    /// Insert a second user (Bob) into an already-seeded DB.
    async fn seed_second_user(db: &Db) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO users (provider, email, name) VALUES ('local','bob@test.local','Bob') \
             RETURNING id",
        )
        .fetch_one(&db.pool)
        .await
        .expect("insert bob")
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn create_get_list_delete(pool: sqlx::PgPool) {
        let db = Db { pool };
        let store = NotebookStore::new(db.clone());
        let (ws, user) = seed(&db).await;

        let id = store.create(ws, user, "My NB").await.unwrap();

        // create seeds exactly one empty code cell
        let detail = store.get_with_cells(id, user).await.unwrap().unwrap();
        assert_eq!(detail.cells.len(), 1);
        assert_eq!(detail.cells[0].cell_type, "code");
        assert_eq!(detail.cells[0].source, "");
        assert!(detail.summary.is_owner, "owner viewer => is_owner true");

        let list = store.list_visible(ws, user, false).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, id);
        assert!(list[0].is_owner);
        assert!(!list[0].bookmarked, "no bookmark by default");
        assert!(list[0].deleted_at.is_none(), "live notebook not trashed");

        store.delete(id).await.unwrap();
        assert!(store.get_with_cells(id, user).await.unwrap().is_none());
        assert!(store
            .list_visible(ws, user, false)
            .await
            .unwrap()
            .is_empty());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn trash_restore_and_bookmark(pool: sqlx::PgPool) {
        let db = Db { pool };
        let store = NotebookStore::new(db.clone());
        let (ws, alice) = seed(&db).await;
        let bob = seed_second_user(&db).await;

        let id = store.create(ws, alice, "nb").await.unwrap();

        // Trash → hidden from the live list, visible in the trash bin.
        store.trash(id).await.unwrap();
        assert!(
            store
                .list_visible(ws, alice, false)
                .await
                .unwrap()
                .is_empty(),
            "trashed notebook hidden from live list"
        );
        let trash = store.list_visible(ws, alice, true).await.unwrap();
        assert_eq!(trash.len(), 1, "trashed notebook shows in trash bin");
        assert!(trash[0].deleted_at.is_some(), "deleted_at populated");

        // Restore → reappears in the live list.
        store.restore(id).await.unwrap();
        let live = store.list_visible(ws, alice, false).await.unwrap();
        assert_eq!(live.len(), 1, "restored notebook back in live list");
        assert!(live[0].deleted_at.is_none());

        // Bookmark by Alice → her list shows bookmarked=true, Bob's does not.
        store.add_bookmark(alice, id).await.unwrap();
        // Make it public so Bob can see it in his list.
        store
            .update(
                id,
                &crate::model::UpdateNotebook {
                    name: None,
                    description: None,
                    is_public: Some(true),
                },
            )
            .await
            .unwrap();

        let alice_list = store.list_visible(ws, alice, false).await.unwrap();
        assert!(
            alice_list.iter().any(|n| n.id == id && n.bookmarked),
            "alice sees her own bookmark"
        );
        let bob_list = store.list_visible(ws, bob, false).await.unwrap();
        assert!(
            bob_list.iter().any(|n| n.id == id && !n.bookmarked),
            "bob does not see alice's bookmark"
        );

        // remove_bookmark is idempotent and clears the flag.
        store.remove_bookmark(alice, id).await.unwrap();
        store.remove_bookmark(alice, id).await.unwrap();
        let alice_list = store.list_visible(ws, alice, false).await.unwrap();
        assert!(alice_list.iter().all(|n| !n.bookmarked));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn set_folder_and_folder_kind(pool: sqlx::PgPool) {
        let db = Db { pool };
        let store = NotebookStore::new(db.clone());
        let (ws, alice) = seed(&db).await;
        let id = store.create(ws, alice, "nb").await.unwrap();

        let fid: Uuid = sqlx::query_scalar(
            "INSERT INTO folders (workspace_id, entity_type, name, created_by) \
             VALUES ($1,'notebook','F',$2) RETURNING id",
        )
        .bind(ws)
        .bind(alice)
        .fetch_one(&db.pool)
        .await
        .unwrap();

        // folder_kind reports the right ws + type.
        let (kw, kt) = store.folder_kind(fid).await.unwrap().unwrap();
        assert_eq!(kw, ws);
        assert_eq!(kt, "notebook");
        assert!(store.folder_kind(Uuid::new_v4()).await.unwrap().is_none());

        // set_folder happy path → folder_id populated in the summary.
        store.set_folder(id, Some(fid)).await.unwrap();
        let (s, _) = store.get_summary(id).await.unwrap().unwrap();
        assert_eq!(s.folder_id, Some(fid));

        // Clear it.
        store.set_folder(id, None).await.unwrap();
        let (s, _) = store.get_summary(id).await.unwrap().unwrap();
        assert!(s.folder_id.is_none());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn list_visibility_owner_and_public(pool: sqlx::PgPool) {
        let db = Db { pool };
        let store = NotebookStore::new(db.clone());
        let (ws, alice) = seed(&db).await;
        let bob = seed_second_user(&db).await;

        let priv_nb = store.create(ws, alice, "priv").await.unwrap();
        let pub_nb = store.create(ws, alice, "pub").await.unwrap();
        // Make pub_nb public.
        store
            .update(
                pub_nb,
                &crate::model::UpdateNotebook {
                    name: None,
                    description: None,
                    is_public: Some(true),
                },
            )
            .await
            .unwrap();

        // Bob sees only the public notebook.
        let bob_list = store.list_visible(ws, bob, false).await.unwrap();
        assert_eq!(bob_list.len(), 1, "bob should see 1 notebook (public only)");
        assert_eq!(bob_list[0].id, pub_nb);
        assert!(!bob_list[0].is_owner, "bob is not the owner");

        // Alice sees both.
        let alice_list = store.list_visible(ws, alice, false).await.unwrap();
        assert_eq!(alice_list.len(), 2, "alice should see both");

        let _ = priv_nb;
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn cell_crud_and_reorder(pool: sqlx::PgPool) {
        let db = Db { pool };
        let store = NotebookStore::new(db.clone());
        let (ws, user) = seed(&db).await;
        let nb = store.create(ws, user, "nb").await.unwrap();

        // create seeds 1 cell; add a second
        let c2 = store.add_cell(nb, "markdown", "# hi", 1).await.unwrap();
        let detail = store.get_with_cells(nb, user).await.unwrap().unwrap();
        let c1 = detail.cells[0].id; // seeded code cell (order 0)
        assert_eq!(detail.cells.len(), 2);

        // Reorder: put c2 first
        store.reorder(nb, &[c2, c1]).await.unwrap();
        let after = store.get_with_cells(nb, user).await.unwrap().unwrap();
        assert_eq!(after.cells[0].id, c2);

        // Delete c1
        store.delete_cell(nb, c1).await.unwrap();
        assert_eq!(
            store
                .get_with_cells(nb, user)
                .await
                .unwrap()
                .unwrap()
                .cells
                .len(),
            1
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn update_notebook_partial(pool: sqlx::PgPool) {
        let db = Db { pool };
        let store = NotebookStore::new(db.clone());
        let (ws, user) = seed(&db).await;
        let nb = store.create(ws, user, "orig").await.unwrap();

        store
            .update(
                nb,
                &crate::model::UpdateNotebook {
                    name: Some("renamed".into()),
                    description: Some("desc".into()),
                    is_public: None,
                },
            )
            .await
            .unwrap();

        let detail = store.get_with_cells(nb, user).await.unwrap().unwrap();
        assert_eq!(detail.summary.name, "renamed");
        assert_eq!(detail.summary.description, "desc");
        let _ = ws;
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn update_cell_partial(pool: sqlx::PgPool) {
        let db = Db { pool };
        let store = NotebookStore::new(db.clone());
        let (ws, user) = seed(&db).await;
        let nb = store.create(ws, user, "nb").await.unwrap();

        let detail = store.get_with_cells(nb, user).await.unwrap().unwrap();
        let cid = detail.cells[0].id;

        store
            .update_cell(
                nb,
                cid,
                &crate::model::UpdateCell {
                    cell_type: Some("sql".into()),
                    source: Some("SELECT 1".into()),
                },
            )
            .await
            .unwrap();

        let after = store.get_with_cells(nb, user).await.unwrap().unwrap();
        assert_eq!(after.cells[0].cell_type, "sql");
        assert_eq!(after.cells[0].source, "SELECT 1");
        let _ = ws;
    }

    // ── Notebook settings tests ──────────────────────────────────────────────

    /// Seed a database for this workspace in the DB. Returns the database id.
    async fn seed_database(db: &Db, ws: Uuid) -> Uuid {
        sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO databases (name, engine, host, port, workspace_id) \
             VALUES ($1, 'trino', 'localhost', 8080, $2) RETURNING id",
        )
        .bind(format!("db-{}", uuid::Uuid::new_v4().simple()))
        .bind(ws)
        .fetch_one(&db.pool)
        .await
        .expect("insert database")
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn engine_options_empty_by_default(pool: sqlx::PgPool) {
        let db = Db { pool };
        let store = NotebookStore::new(db.clone());
        let (ws, _user) = seed(&db).await;

        let opts = store.list_engine_options(ws).await.unwrap();
        assert!(opts.is_empty(), "no databases seeded → empty list");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn engine_options_enabled_toggling(pool: sqlx::PgPool) {
        let db = Db { pool };
        let store = NotebookStore::new(db.clone());
        let (ws, _user) = seed(&db).await;
        let db_id = seed_database(&db, ws).await;

        // Before enabling: enabled=false.
        let opts = store.list_engine_options(ws).await.unwrap();
        assert_eq!(opts.len(), 1);
        assert!(!opts[0].enabled, "not enabled yet");

        // Enable it.
        store.set_engine_access(ws, &[db_id]).await.unwrap();
        let opts = store.list_engine_options(ws).await.unwrap();
        assert_eq!(opts.len(), 1);
        assert!(opts[0].enabled, "should now be enabled");
        assert_eq!(opts[0].id, db_id);

        // Disable it (empty set).
        store.set_engine_access(ws, &[]).await.unwrap();
        let opts = store.list_engine_options(ws).await.unwrap();
        assert!(!opts[0].enabled, "disabled after empty set");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn set_engine_access_replaces_set(pool: sqlx::PgPool) {
        let db = Db { pool };
        let store = NotebookStore::new(db.clone());
        let (ws, _user) = seed(&db).await;
        let db1 = seed_database(&db, ws).await;
        let db2 = seed_database(&db, ws).await;

        // Enable both.
        store.set_engine_access(ws, &[db1, db2]).await.unwrap();
        let opts = store.list_engine_options(ws).await.unwrap();
        assert!(opts.iter().all(|o| o.enabled), "both enabled");

        // Replace with just db2 — db1 should no longer be enabled.
        store.set_engine_access(ws, &[db2]).await.unwrap();
        let opts = store.list_engine_options(ws).await.unwrap();
        let db1_opt = opts.iter().find(|o| o.id == db1).unwrap();
        let db2_opt = opts.iter().find(|o| o.id == db2).unwrap();
        assert!(!db1_opt.enabled, "db1 should no longer be enabled");
        assert!(db2_opt.enabled, "db2 should still be enabled");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn set_engine_access_cross_workspace_rejected(pool: sqlx::PgPool) {
        let db = Db { pool };
        let store = NotebookStore::new(db.clone());
        let (ws_a, _user_a) = seed(&db).await;
        // seed a second workspace with a second user (needs unique email)
        let user_b: Uuid = sqlx::query_scalar(
            "INSERT INTO users (provider, email, name) VALUES ('local','b@test.local','B') RETURNING id",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let ws_b: Uuid = sqlx::query_scalar(
            "INSERT INTO workspaces (slug, name, created_by) VALUES ('ws-b','B',$1) RETURNING id",
        )
        .bind(user_b)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let db_b = seed_database(&db, ws_b).await;

        // Trying to enable ws_b's database in ws_a → InvalidInput (400).
        let result = store.set_engine_access(ws_a, &[db_b]).await;
        assert!(result.is_err(), "cross-workspace id should be rejected");
        // Access table for ws_a must be unchanged.
        let opts = store.list_engine_options(ws_a).await.unwrap();
        assert!(
            opts.is_empty(),
            "ws_a has no databases, access table unchanged"
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn get_settings_combines_engines_and_catalogs(pool: sqlx::PgPool) {
        let db = Db { pool };
        let store = NotebookStore::new(db.clone());
        let (ws, _user) = seed(&db).await;
        let db_id = seed_database(&db, ws).await;

        // Enable the engine.
        store.set_engine_access(ws, &[db_id]).await.unwrap();

        let settings = store.get_settings(ws).await.unwrap();
        assert_eq!(settings.engines.len(), 1);
        assert!(settings.engines[0].enabled);
        // No catalogs (workspace_warehouses) in this workspace.
        assert!(settings.catalogs.is_empty());
    }

    /// Seed a workspace warehouse (notebook "catalog") for this workspace. Returns its id.
    async fn seed_warehouse(db: &Db, ws: Uuid) -> Uuid {
        sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO workspace_warehouses (workspace_id, warehouse_id, name) \
             VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(ws)
        .bind(Uuid::new_v4())
        .bind(format!("wh-{}", Uuid::new_v4().simple()))
        .fetch_one(&db.pool)
        .await
        .expect("insert warehouse")
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn catalog_options_enabled_toggling(pool: sqlx::PgPool) {
        let db = Db { pool };
        let store = NotebookStore::new(db.clone());
        let (ws, _user) = seed(&db).await;
        let wh = seed_warehouse(&db, ws).await;

        // Before enabling: enabled=false.
        let opts = store.list_catalog_options(ws).await.unwrap();
        assert_eq!(opts.len(), 1);
        assert!(!opts[0].enabled, "not enabled yet");
        assert_eq!(opts[0].id, wh);

        // Enable it.
        store.set_catalog_access(ws, &[wh]).await.unwrap();
        let opts = store.list_catalog_options(ws).await.unwrap();
        assert!(opts[0].enabled, "should now be enabled");

        // Replace with empty set → disabled again.
        store.set_catalog_access(ws, &[]).await.unwrap();
        let opts = store.list_catalog_options(ws).await.unwrap();
        assert!(!opts[0].enabled, "disabled after empty set");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn record_output_persists_capped_outputs(pool: sqlx::PgPool) {
        use crate::output::CellOutput;
        let db = Db { pool };
        let store = NotebookStore::new(db.clone());
        let (ws, user) = seed(&db).await;
        let nb = store.create(ws, user, "nb").await.unwrap();
        let cid = store.add_cell(nb, "code", "", 1).await.unwrap();
        let outs = vec![CellOutput::Stream {
            name: "stdout".into(),
            text: "hi".into(),
        }];
        store
            .record_output(nb, cid, &outs, Some(3), Some(42))
            .await
            .unwrap();
        let (lo, ec, ms): (Option<serde_json::Value>, Option<i32>, Option<i32>) = sqlx::query_as(
            "SELECT last_output, execution_count, last_execution_time_ms \
             FROM notebook_cells WHERE id=$1",
        )
        .bind(cid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(ec, Some(3));
        assert_eq!(ms, Some(42));
        assert_eq!(lo.unwrap()[0]["type"], "stream");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn set_catalog_access_cross_workspace_rejected(pool: sqlx::PgPool) {
        let db = Db { pool };
        let store = NotebookStore::new(db.clone());
        let (ws_a, _user_a) = seed(&db).await;
        // Second workspace owned by a second user (unique email + slug).
        let user_b: Uuid = sqlx::query_scalar(
            "INSERT INTO users (provider, email, name) VALUES ('local','bcat@test.local','B') RETURNING id",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let ws_b: Uuid = sqlx::query_scalar(
            "INSERT INTO workspaces (slug, name, created_by) VALUES ('ws-bcat','BCat',$1) RETURNING id",
        )
        .bind(user_b)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let wh_b = seed_warehouse(&db, ws_b).await;

        // Enabling ws_b's warehouse in ws_a → rejected, ws_a access table untouched.
        let result = store.set_catalog_access(ws_a, &[wh_b]).await;
        assert!(
            result.is_err(),
            "cross-workspace catalog id must be rejected"
        );
        let opts = store.list_catalog_options(ws_a).await.unwrap();
        assert!(
            opts.is_empty(),
            "ws_a has no warehouses; access table unchanged"
        );
    }
}
