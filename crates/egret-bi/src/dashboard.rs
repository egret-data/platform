//! Dashboard repository: a dashboard row plus its placed cards (grid layout).
//!
//! Card layout is saved whole-list: the client sends the full set of tabs + cards on each
//! update and `replace_layout` swaps them in a transaction (Metabase-style layout PUT),
//! which keeps grid edits atomic and avoids per-card diffing.

use egret_metastore::Db;
use uuid::Uuid;

use crate::error::BiError;

#[derive(Clone)]
pub struct DashboardRepo {
    db: Db,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Dashboard {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub created_by: Option<Uuid>,
    pub folder_id: Option<Uuid>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub width: String,
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DashboardCard {
    pub id: Uuid,
    pub dashboard_id: Uuid,
    pub chart_id: Option<Uuid>,
    pub card_type: String,
    pub grid_col: i32,
    pub grid_row: i32,
    pub size_x: i32,
    pub size_y: i32,
    pub options: serde_json::Value,
    pub ordinal: i32,
    pub tab_id: Option<Uuid>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DashboardTab {
    pub id: Uuid,
    pub name: String,
    pub position: i32,
}

/// A tab as supplied by the client when saving a layout (no server id yet).
#[derive(Debug, Clone)]
pub struct NewTab {
    pub name: String,
    pub position: i32,
}

/// Private struct for list() — sqlx tuple FromRow is version-sensitive; use flatten instead.
#[derive(sqlx::FromRow)]
struct DashboardWithCount {
    #[sqlx(flatten)]
    dash: Dashboard,
    card_count: i64,
}

#[derive(sqlx::FromRow)]
struct DashboardWithCountBm {
    #[sqlx(flatten)]
    dash: Dashboard,
    card_count: i64,
    bookmarked: bool,
}

/// A card as supplied by the client when saving a layout (no server id yet).
#[derive(Debug, Clone)]
pub struct NewCard {
    pub chart_id: Option<Uuid>,
    pub card_type: String,
    pub grid_col: i32,
    pub grid_row: i32,
    pub size_x: i32,
    pub size_y: i32,
    pub options: serde_json::Value,
    /// 0-based index into the layout's `tabs`; `None` for a tabless dashboard.
    pub tab_index: Option<usize>,
}

const COLS: &str =
    "id, workspace_id, name, description, parameters, created_by, folder_id, updated_at, width, deleted_at";
const CARD_COLS: &str =
    "id, dashboard_id, chart_id, card_type, grid_col, grid_row, size_x, size_y, options, ordinal, tab_id";

/// Map a unique-violation to a Conflict; pass everything else through.
fn map_conflict_err(e: sqlx::Error) -> BiError {
    if let sqlx::Error::Database(ref dbe) = e {
        if dbe.is_unique_violation() {
            return BiError::Conflict("a dashboard with that name already exists".into());
        }
    }
    BiError::Db(e)
}

impl DashboardRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub async fn create(
        &self,
        workspace_id: Uuid,
        name: &str,
        description: &str,
        created_by: Option<Uuid>,
    ) -> Result<Dashboard, BiError> {
        let sql = format!(
            "INSERT INTO dashboards (workspace_id, name, description, created_by) \
             VALUES ($1,$2,$3,$4) RETURNING {COLS}"
        );
        sqlx::query_as::<_, Dashboard>(&sql)
            .bind(workspace_id)
            .bind(name)
            .bind(description)
            .bind(created_by)
            .fetch_one(&self.db.pool)
            .await
            .map_err(map_conflict_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<Dashboard>, BiError> {
        let sql = format!("SELECT {COLS} FROM dashboards WHERE id = $1");
        Ok(sqlx::query_as::<_, Dashboard>(&sql)
            .bind(id)
            .fetch_optional(&self.db.pool)
            .await?)
    }

    /// Dashboards in a workspace, each with its card count, newest first.
    /// `trashed` selects the trash bin (soft-deleted) instead of the live list.
    pub async fn list_by_workspace(
        &self,
        workspace_id: Uuid,
        trashed: bool,
    ) -> Result<Vec<(Dashboard, i64)>, BiError> {
        let pred = if trashed {
            "deleted_at IS NOT NULL"
        } else {
            "deleted_at IS NULL"
        };
        let sql = format!(
            "SELECT {COLS}, \
             (SELECT count(*) FROM dashboard_cards c WHERE c.dashboard_id = dashboards.id) AS card_count \
             FROM dashboards WHERE workspace_id = $1 AND {pred} ORDER BY updated_at DESC"
        );
        let rows = sqlx::query_as::<_, DashboardWithCount>(&sql)
            .bind(workspace_id)
            .fetch_all(&self.db.pool)
            .await?;
        Ok(rows.into_iter().map(|r| (r.dash, r.card_count)).collect())
    }

    /// Soft-delete (trashed=true → deleted_at=now()) or restore (false → NULL).
    pub async fn set_trashed(&self, id: Uuid, trashed: bool) -> Result<(), BiError> {
        let sql = if trashed {
            "UPDATE dashboards SET deleted_at = now() WHERE id = $1"
        } else {
            "UPDATE dashboards SET deleted_at = NULL WHERE id = $1"
        };
        sqlx::query(sql)
            .bind(id)
            .execute(&self.db.pool)
            .await
            .map_err(map_conflict_err)?;
        Ok(())
    }

    /// Update dashboard metadata only (name/description). Card layout goes through
    /// `replace_layout`; callers that change both run them together.
    pub async fn update(
        &self,
        id: Uuid,
        name: &str,
        description: &str,
        parameters: &serde_json::Value,
        width: &str,
    ) -> Result<(), BiError> {
        sqlx::query(
            "UPDATE dashboards SET name=$2, description=$3, parameters=$4, width=$5, updated_at=now() WHERE id=$1",
        )
        .bind(id)
        .bind(name)
        .bind(description)
        .bind(parameters)
        .bind(width)
        .execute(&self.db.pool)
        .await
        .map_err(map_conflict_err)?;
        Ok(())
    }

    /// Permanently delete a dashboard. Cards/tabs cascade via FK; polymorphic bookmark rows do not, so purge them in the same transaction.
    pub async fn delete(&self, id: Uuid) -> Result<(), BiError> {
        let mut tx = self.db.pool.begin().await?;
        // bookmarks are polymorphic (no FK on entity_id) so they don't cascade — purge manually.
        sqlx::query("DELETE FROM bookmarks WHERE entity_type='dashboard' AND entity_id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM dashboards WHERE id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn set_folder(&self, id: Uuid, folder_id: Option<Uuid>) -> Result<(), BiError> {
        sqlx::query("UPDATE dashboards SET folder_id=$2 WHERE id=$1")
            .bind(id)
            .bind(folder_id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    pub async fn add_bookmark(
        &self,
        user: Uuid,
        entity_type: &str,
        entity_id: Uuid,
    ) -> Result<(), BiError> {
        sqlx::query(
            "INSERT INTO bookmarks (user_id, entity_type, entity_id) VALUES ($1,$2,$3) \
             ON CONFLICT (user_id, entity_type, entity_id) DO NOTHING",
        )
        .bind(user)
        .bind(entity_type)
        .bind(entity_id)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    pub async fn remove_bookmark(
        &self,
        user: Uuid,
        entity_type: &str,
        entity_id: Uuid,
    ) -> Result<(), BiError> {
        sqlx::query("DELETE FROM bookmarks WHERE user_id=$1 AND entity_type=$2 AND entity_id=$3")
            .bind(user)
            .bind(entity_type)
            .bind(entity_id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    pub async fn is_bookmarked(&self, user: Uuid, id: Uuid) -> Result<bool, BiError> {
        Ok(sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM bookmarks WHERE user_id=$1 AND entity_type='dashboard' AND entity_id=$2)",
        )
        .bind(user)
        .bind(id)
        .fetch_one(&self.db.pool)
        .await?)
    }

    /// Like `list_by_workspace` but also returns a per-user `bookmarked` flag.
    pub async fn list_by_workspace_for(
        &self,
        workspace_id: Uuid,
        user: Uuid,
        trashed: bool,
    ) -> Result<Vec<(Dashboard, i64, bool)>, BiError> {
        let pred = if trashed {
            "deleted_at IS NOT NULL"
        } else {
            "deleted_at IS NULL"
        };
        let sql = format!(
            "SELECT {COLS}, \
             (SELECT count(*) FROM dashboard_cards c WHERE c.dashboard_id = dashboards.id) AS card_count, \
             EXISTS (SELECT 1 FROM bookmarks b WHERE b.user_id = $2 AND b.entity_type='dashboard' AND b.entity_id = dashboards.id) AS bookmarked \
             FROM dashboards WHERE workspace_id = $1 AND {pred} ORDER BY updated_at DESC"
        );
        let rows = sqlx::query_as::<_, DashboardWithCountBm>(&sql)
            .bind(workspace_id)
            .bind(user)
            .fetch_all(&self.db.pool)
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.dash, r.card_count, r.bookmarked))
            .collect())
    }

    /// All cards on a dashboard, in placement order.
    pub async fn cards(&self, dashboard_id: Uuid) -> Result<Vec<DashboardCard>, BiError> {
        let sql = format!(
            "SELECT {CARD_COLS} FROM dashboard_cards WHERE dashboard_id = $1 ORDER BY ordinal, grid_row, grid_col"
        );
        Ok(sqlx::query_as::<_, DashboardCard>(&sql)
            .bind(dashboard_id)
            .fetch_all(&self.db.pool)
            .await?)
    }

    /// All tabs on a dashboard, ordered by position.
    pub async fn tabs(&self, dashboard_id: Uuid) -> Result<Vec<DashboardTab>, BiError> {
        Ok(sqlx::query_as::<_, DashboardTab>(
            "SELECT id, name, position FROM dashboard_tabs WHERE dashboard_id = $1 ORDER BY position, id",
        )
        .bind(dashboard_id)
        .fetch_all(&self.db.pool)
        .await?)
    }

    /// Atomically replace the dashboard's full tab + card set (in supplied order).
    /// Cards reference their tab by `tab_index` into `tabs`; `None` means no tab.
    pub async fn replace_layout(
        &self,
        dashboard_id: Uuid,
        tabs: &[NewTab],
        cards: &[NewCard],
    ) -> Result<(), BiError> {
        for c in cards {
            match c.tab_index {
                Some(i) if i >= tabs.len() => {
                    return Err(BiError::BadRequest(format!(
                        "card tab_index {i} out of range (0..{})",
                        tabs.len()
                    )));
                }
                _ => {}
            }
        }

        let mut tx = self.db.pool.begin().await?;
        sqlx::query("DELETE FROM dashboard_cards WHERE dashboard_id = $1")
            .bind(dashboard_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM dashboard_tabs WHERE dashboard_id = $1")
            .bind(dashboard_id)
            .execute(&mut *tx)
            .await?;

        let mut tab_ids: Vec<Uuid> = Vec::with_capacity(tabs.len());
        for t in tabs {
            let id: Uuid = sqlx::query_scalar(
                "INSERT INTO dashboard_tabs (dashboard_id, name, position) VALUES ($1,$2,$3) RETURNING id",
            )
            .bind(dashboard_id).bind(&t.name).bind(t.position)
            .fetch_one(&mut *tx).await?;
            tab_ids.push(id);
        }

        for (i, c) in cards.iter().enumerate() {
            let tab_id = c.tab_index.map(|idx| tab_ids[idx]);
            sqlx::query(
                "INSERT INTO dashboard_cards \
                 (dashboard_id, chart_id, card_type, grid_col, grid_row, size_x, size_y, options, ordinal, tab_id) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
            )
            .bind(dashboard_id).bind(c.chart_id).bind(&c.card_type)
            .bind(c.grid_col).bind(c.grid_row).bind(c.size_x).bind(c.size_y)
            .bind(&c.options).bind(i as i32).bind(tab_id)
            .execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// The workspace a dashboard belongs to (for the nested-ownership guard). None if absent.
    pub async fn dashboard_workspace(&self, id: Uuid) -> Result<Option<Uuid>, BiError> {
        Ok(
            sqlx::query_scalar::<_, Uuid>("SELECT workspace_id FROM dashboards WHERE id=$1")
                .bind(id)
                .fetch_optional(&self.db.pool)
                .await?,
        )
    }

    /// The owner (created_by) of a dashboard, for owner-or-admin checks.
    pub async fn owner(&self, id: Uuid) -> Result<Option<Uuid>, BiError> {
        Ok(
            sqlx::query_scalar::<_, Option<Uuid>>("SELECT created_by FROM dashboards WHERE id=$1")
                .bind(id)
                .fetch_optional(&self.db.pool)
                .await?
                .flatten(),
        )
    }

    /// Deep-copy a dashboard (row + tabs + cards) under a new owner, in one transaction.
    /// The copy's name is "<name> (copy)"; tab references on cards are remapped to the copy's tabs.
    /// Returns None if the source doesn't exist.
    pub async fn duplicate(
        &self,
        src_id: Uuid,
        new_owner: Option<Uuid>,
    ) -> Result<Option<Dashboard>, BiError> {
        let Some(src) = self.get(src_id).await? else {
            return Ok(None);
        };
        let src_tabs = self.tabs(src_id).await?;
        let src_cards = self.cards(src_id).await?;

        let mut tx = self.db.pool.begin().await?;
        let copy: Dashboard = sqlx::query_as::<_, Dashboard>(&format!(
            "INSERT INTO dashboards (workspace_id, name, description, parameters, created_by, width) \
             VALUES ($1,$2,$3,$4,$5,$6) RETURNING {COLS}"
        ))
        .bind(src.workspace_id)
        .bind(format!("{} (copy)", src.name))
        .bind(&src.description)
        .bind(&src.parameters)
        .bind(new_owner)
        .bind(&src.width)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_conflict_err)?;

        let mut tab_map: std::collections::HashMap<Uuid, Uuid> = std::collections::HashMap::new();
        for t in &src_tabs {
            let nid: Uuid = sqlx::query_scalar(
                "INSERT INTO dashboard_tabs (dashboard_id, name, position) VALUES ($1,$2,$3) RETURNING id",
            )
            .bind(copy.id).bind(&t.name).bind(t.position)
            .fetch_one(&mut *tx).await?;
            tab_map.insert(t.id, nid);
        }
        for c in &src_cards {
            let new_tab = c.tab_id.and_then(|old| tab_map.get(&old).copied());
            sqlx::query(
                "INSERT INTO dashboard_cards \
                 (dashboard_id, chart_id, card_type, grid_col, grid_row, size_x, size_y, options, ordinal, tab_id) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
            )
            .bind(copy.id).bind(c.chart_id).bind(&c.card_type)
            .bind(c.grid_col).bind(c.grid_row).bind(c.size_x).bind(c.size_y)
            .bind(&c.options).bind(c.ordinal).bind(new_tab)
            .execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(Some(copy))
    }
}
