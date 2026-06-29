//! `NotebookStore` — parameterized Postgres persistence for notebook documents
//! and their ordered cells.

use egret_metastore::Db;
use uuid::Uuid;

use crate::error::NotebookApiError;
use crate::model::{Cell, NotebookDetail, NotebookSummary, UpdateCell, UpdateNotebook};

// ── Row structs ──────────────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct NotebookRow {
    id: Uuid,
    workspace_id: Uuid,
    owner_user_id: Uuid,
    name: String,
    description: String,
    engine: String,
    is_public: bool,
    updated_at: chrono::DateTime<chrono::Utc>,
}

impl NotebookRow {
    fn into_summary(self, viewer: Uuid) -> NotebookSummary {
        let is_owner = self.owner_user_id == viewer;
        NotebookSummary {
            id: self.id,
            name: self.name,
            description: self.description,
            engine: self.engine,
            is_public: self.is_public,
            owner_user_id: self.owner_user_id,
            is_owner,
            updated_at: self.updated_at,
        }
    }

    /// Convert without a viewer (ownership unknown at the store level).
    fn into_summary_unknown_owner(self) -> (NotebookSummary, Uuid) {
        let ws = self.workspace_id;
        let owner = self.owner_user_id;
        let summary = NotebookSummary {
            id: self.id,
            name: self.name,
            description: self.description,
            engine: self.engine,
            is_public: self.is_public,
            owner_user_id: owner,
            // caller (route) sets this
            is_owner: false,
            updated_at: self.updated_at,
        };
        (summary, ws)
    }
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

const NB_COLS: &str =
    "id, workspace_id, owner_user_id, name, description, engine, is_public, updated_at";

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
        engine: &str,
    ) -> Result<Uuid, NotebookApiError> {
        let mut tx = self.db.pool.begin().await?;
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO notebooks (workspace_id, owner_user_id, name, engine) \
             VALUES ($1,$2,$3,$4) RETURNING id",
        )
        .bind(ws)
        .bind(owner)
        .bind(name)
        .bind(engine)
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
    pub async fn list_visible(
        &self,
        ws: Uuid,
        viewer: Uuid,
    ) -> Result<Vec<NotebookSummary>, NotebookApiError> {
        let rows = sqlx::query_as::<_, NotebookRow>(&format!(
            "SELECT {NB_COLS} FROM notebooks \
             WHERE workspace_id=$1 AND (owner_user_id=$2 OR is_public) \
             ORDER BY updated_at DESC"
        ))
        .bind(ws)
        .bind(viewer)
        .fetch_all(&self.db.pool)
        .await?;
        Ok(rows.into_iter().map(|r| r.into_summary(viewer)).collect())
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
             engine      = COALESCE($4, engine), \
             is_public   = COALESCE($5, is_public), \
             updated_at  = now() \
             WHERE id=$1",
        )
        .bind(id)
        .bind(p.name.as_deref())
        .bind(p.description.as_deref())
        .bind(p.engine.as_deref())
        .bind(p.is_public)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// Delete a notebook (cascades to cells via FK).
    pub async fn delete(&self, id: Uuid) -> Result<(), NotebookApiError> {
        sqlx::query("DELETE FROM notebooks WHERE id=$1")
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

        let id = store.create(ws, user, "My NB", "daft").await.unwrap();

        // create seeds exactly one empty code cell
        let detail = store.get_with_cells(id, user).await.unwrap().unwrap();
        assert_eq!(detail.cells.len(), 1);
        assert_eq!(detail.cells[0].cell_type, "code");
        assert_eq!(detail.cells[0].source, "");
        assert!(detail.summary.is_owner, "owner viewer => is_owner true");

        let list = store.list_visible(ws, user).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, id);
        assert!(list[0].is_owner);

        store.delete(id).await.unwrap();
        assert!(store.get_with_cells(id, user).await.unwrap().is_none());
        assert!(store.list_visible(ws, user).await.unwrap().is_empty());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn list_visibility_owner_and_public(pool: sqlx::PgPool) {
        let db = Db { pool };
        let store = NotebookStore::new(db.clone());
        let (ws, alice) = seed(&db).await;
        let bob = seed_second_user(&db).await;

        let priv_nb = store.create(ws, alice, "priv", "daft").await.unwrap();
        let pub_nb = store.create(ws, alice, "pub", "daft").await.unwrap();
        // Make pub_nb public.
        store
            .update(
                pub_nb,
                &crate::model::UpdateNotebook {
                    name: None,
                    description: None,
                    engine: None,
                    is_public: Some(true),
                },
            )
            .await
            .unwrap();

        // Bob sees only the public notebook.
        let bob_list = store.list_visible(ws, bob).await.unwrap();
        assert_eq!(bob_list.len(), 1, "bob should see 1 notebook (public only)");
        assert_eq!(bob_list[0].id, pub_nb);
        assert!(!bob_list[0].is_owner, "bob is not the owner");

        // Alice sees both.
        let alice_list = store.list_visible(ws, alice).await.unwrap();
        assert_eq!(alice_list.len(), 2, "alice should see both");

        let _ = priv_nb;
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn cell_crud_and_reorder(pool: sqlx::PgPool) {
        let db = Db { pool };
        let store = NotebookStore::new(db.clone());
        let (ws, user) = seed(&db).await;
        let nb = store.create(ws, user, "nb", "daft").await.unwrap();

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
        let nb = store.create(ws, user, "orig", "daft").await.unwrap();

        store
            .update(
                nb,
                &crate::model::UpdateNotebook {
                    name: Some("renamed".into()),
                    description: Some("desc".into()),
                    engine: None,
                    is_public: None,
                },
            )
            .await
            .unwrap();

        let detail = store.get_with_cells(nb, user).await.unwrap().unwrap();
        assert_eq!(detail.summary.name, "renamed");
        assert_eq!(detail.summary.description, "desc");
        assert_eq!(detail.summary.engine, "daft"); // unchanged
        let _ = ws;
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn update_cell_partial(pool: sqlx::PgPool) {
        let db = Db { pool };
        let store = NotebookStore::new(db.clone());
        let (ws, user) = seed(&db).await;
        let nb = store.create(ws, user, "nb", "daft").await.unwrap();

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
}
