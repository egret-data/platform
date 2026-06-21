//! Dashboard repository: a dashboard row plus its placed cards (grid layout).
//!
//! Card layout is saved whole-list: the client sends the full set of cards on each
//! update and `replace_cards` swaps them in a transaction (Metabase-style layout PUT),
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
    pub created_by: Option<Uuid>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
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
}

/// Private struct for list() — sqlx tuple FromRow is version-sensitive; use flatten instead.
#[derive(sqlx::FromRow)]
struct DashboardWithCount {
    #[sqlx(flatten)]
    dash: Dashboard,
    card_count: i64,
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
}

const COLS: &str = "id, workspace_id, name, description, created_by, updated_at";
const CARD_COLS: &str =
    "id, dashboard_id, chart_id, card_type, grid_col, grid_row, size_x, size_y, options, ordinal";

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
    pub async fn list_by_workspace(
        &self,
        workspace_id: Uuid,
    ) -> Result<Vec<(Dashboard, i64)>, BiError> {
        let sql = format!(
            "SELECT {COLS}, \
             (SELECT count(*) FROM dashboard_cards c WHERE c.dashboard_id = dashboards.id) AS card_count \
             FROM dashboards WHERE workspace_id = $1 ORDER BY updated_at DESC"
        );
        let rows = sqlx::query_as::<_, DashboardWithCount>(&sql)
            .bind(workspace_id)
            .fetch_all(&self.db.pool)
            .await?;
        Ok(rows.into_iter().map(|r| (r.dash, r.card_count)).collect())
    }

    /// Update dashboard metadata only (name/description). Card layout goes through
    /// `replace_cards`; callers that change both run them together.
    pub async fn update_meta(
        &self,
        id: Uuid,
        name: &str,
        description: &str,
    ) -> Result<(), BiError> {
        sqlx::query("UPDATE dashboards SET name=$2, description=$3, updated_at=now() WHERE id=$1")
            .bind(id)
            .bind(name)
            .bind(description)
            .execute(&self.db.pool)
            .await
            .map_err(map_conflict_err)?;
        Ok(())
    }

    pub async fn delete(&self, id: Uuid) -> Result<(), BiError> {
        sqlx::query("DELETE FROM dashboards WHERE id=$1")
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
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

    /// Atomically replace the dashboard's full card set with `cards` (in supplied order).
    pub async fn replace_cards(
        &self,
        dashboard_id: Uuid,
        cards: &[NewCard],
    ) -> Result<(), BiError> {
        let mut tx = self.db.pool.begin().await?;
        sqlx::query("DELETE FROM dashboard_cards WHERE dashboard_id = $1")
            .bind(dashboard_id)
            .execute(&mut *tx)
            .await?;
        for (i, c) in cards.iter().enumerate() {
            sqlx::query(
                "INSERT INTO dashboard_cards \
                 (dashboard_id, chart_id, card_type, grid_col, grid_row, size_x, size_y, options, ordinal) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
            )
            .bind(dashboard_id)
            .bind(c.chart_id)
            .bind(&c.card_type)
            .bind(c.grid_col)
            .bind(c.grid_row)
            .bind(c.size_x)
            .bind(c.size_y)
            .bind(&c.options)
            .bind(i as i32)
            .execute(&mut *tx)
            .await?;
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
}
