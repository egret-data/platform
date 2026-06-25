//! Chart repository: chart rows (viz_type + JSONB params) scoped to a workspace + dataset.

use egret_metastore::Db;
use uuid::Uuid;

use crate::error::BiError;

#[derive(Clone)]
pub struct ChartRepo {
    db: Db,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Chart {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub dataset_id: Uuid,
    pub name: String,
    pub description: String,
    pub viz_type: String,
    pub params: serde_json::Value,
    pub created_by: Option<Uuid>,
    pub folder_id: Option<Uuid>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Private struct for list_by_workspace_for — sqlx tuple FromRow is version-sensitive; use flatten instead.
#[derive(sqlx::FromRow)]
struct ChartWithBm {
    #[sqlx(flatten)]
    chart: Chart,
    bookmarked: bool,
}

const COLS: &str =
    "id, workspace_id, dataset_id, name, description, viz_type, params, created_by, folder_id, updated_at, deleted_at";

/// Map a unique-violation (on insert or update) to a Conflict; pass everything else through.
fn map_conflict_err(e: sqlx::Error) -> BiError {
    if let sqlx::Error::Database(ref dbe) = e {
        if dbe.is_unique_violation() {
            return BiError::Conflict("a chart with that name already exists".into());
        }
    }
    BiError::Db(e)
}

impl ChartRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        workspace_id: Uuid,
        dataset_id: Uuid,
        name: &str,
        description: &str,
        viz_type: &str,
        params: &serde_json::Value,
        created_by: Option<Uuid>,
    ) -> Result<Chart, BiError> {
        let sql = format!(
            "INSERT INTO charts (workspace_id, dataset_id, name, description, viz_type, params, created_by) \
             VALUES ($1,$2,$3,$4,$5,$6,$7) RETURNING {COLS}"
        );
        sqlx::query_as::<_, Chart>(&sql)
            .bind(workspace_id)
            .bind(dataset_id)
            .bind(name)
            .bind(description)
            .bind(viz_type)
            .bind(params)
            .bind(created_by)
            .fetch_one(&self.db.pool)
            .await
            .map_err(map_conflict_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<Chart>, BiError> {
        let sql = format!("SELECT {COLS} FROM charts WHERE id = $1");
        Ok(sqlx::query_as::<_, Chart>(&sql)
            .bind(id)
            .fetch_optional(&self.db.pool)
            .await?)
    }

    pub async fn list_by_workspace(&self, workspace_id: Uuid) -> Result<Vec<Chart>, BiError> {
        let sql =
            format!("SELECT {COLS} FROM charts WHERE workspace_id = $1 ORDER BY updated_at DESC");
        Ok(sqlx::query_as::<_, Chart>(&sql)
            .bind(workspace_id)
            .fetch_all(&self.db.pool)
            .await?)
    }

    /// Charts in a workspace with a per-user `bookmarked` flag, newest first.
    /// `trashed` selects the trash bin (soft-deleted) instead of the live list.
    pub async fn list_by_workspace_for(
        &self,
        workspace_id: Uuid,
        user: Uuid,
        trashed: bool,
    ) -> Result<Vec<(Chart, bool)>, BiError> {
        let pred = if trashed {
            "deleted_at IS NOT NULL"
        } else {
            "deleted_at IS NULL"
        };
        let sql = format!(
            "SELECT {COLS}, \
             EXISTS (SELECT 1 FROM bookmarks b WHERE b.user_id = $2 AND b.entity_type='chart' AND b.entity_id = charts.id) AS bookmarked \
             FROM charts WHERE workspace_id = $1 AND {pred} ORDER BY updated_at DESC"
        );
        let rows = sqlx::query_as::<_, ChartWithBm>(&sql)
            .bind(workspace_id)
            .bind(user)
            .fetch_all(&self.db.pool)
            .await?;
        Ok(rows.into_iter().map(|r| (r.chart, r.bookmarked)).collect())
    }

    /// Soft-delete (trashed=true → deleted_at=now()) or restore (false → NULL).
    pub async fn set_trashed(&self, id: Uuid, trashed: bool) -> Result<(), BiError> {
        let sql = if trashed {
            "UPDATE charts SET deleted_at = now() WHERE id = $1"
        } else {
            "UPDATE charts SET deleted_at = NULL WHERE id = $1"
        };
        sqlx::query(sql)
            .bind(id)
            .execute(&self.db.pool)
            .await
            .map_err(map_conflict_err)?;
        Ok(())
    }

    pub async fn add_bookmark(&self, user: Uuid, entity_id: Uuid) -> Result<(), BiError> {
        sqlx::query(
            "INSERT INTO bookmarks (user_id, entity_type, entity_id) VALUES ($1,'chart',$2) \
             ON CONFLICT (user_id, entity_type, entity_id) DO NOTHING",
        )
        .bind(user)
        .bind(entity_id)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    pub async fn remove_bookmark(&self, user: Uuid, entity_id: Uuid) -> Result<(), BiError> {
        sqlx::query(
            "DELETE FROM bookmarks WHERE user_id=$1 AND entity_type='chart' AND entity_id=$2",
        )
        .bind(user)
        .bind(entity_id)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    pub async fn update(
        &self,
        id: Uuid,
        dataset_id: Uuid,
        name: &str,
        description: &str,
        viz_type: &str,
        params: &serde_json::Value,
    ) -> Result<(), BiError> {
        sqlx::query(
            "UPDATE charts SET dataset_id=$2, name=$3, description=$4, viz_type=$5, params=$6, updated_at=now() WHERE id=$1",
        )
        .bind(id)
        .bind(dataset_id)
        .bind(name)
        .bind(description)
        .bind(viz_type)
        .bind(params)
        .execute(&self.db.pool)
        .await
        .map_err(map_conflict_err)?;
        Ok(())
    }

    pub async fn set_folder(&self, id: Uuid, folder_id: Option<Uuid>) -> Result<(), BiError> {
        sqlx::query("UPDATE charts SET folder_id=$2 WHERE id=$1")
            .bind(id)
            .bind(folder_id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    pub async fn delete(&self, id: Uuid) -> Result<(), BiError> {
        sqlx::query("DELETE FROM charts WHERE id=$1")
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    /// The workspace a chart belongs to (for the nested-ownership guard). None if absent.
    pub async fn chart_workspace(&self, id: Uuid) -> Result<Option<Uuid>, BiError> {
        Ok(
            sqlx::query_scalar::<_, Uuid>("SELECT workspace_id FROM charts WHERE id=$1")
                .bind(id)
                .fetch_optional(&self.db.pool)
                .await?,
        )
    }

    /// The owner (created_by) of a chart, for owner-or-admin checks.
    pub async fn owner(&self, id: Uuid) -> Result<Option<Uuid>, BiError> {
        Ok(
            sqlx::query_scalar::<_, Option<Uuid>>("SELECT created_by FROM charts WHERE id=$1")
                .bind(id)
                .fetch_optional(&self.db.pool)
                .await?
                .flatten(),
        )
    }

    /// All chart names already used in a workspace (for collision-free copy naming).
    pub async fn names_in_workspace(&self, workspace_id: Uuid) -> Result<Vec<String>, BiError> {
        Ok(
            sqlx::query_scalar::<_, String>("SELECT name FROM charts WHERE workspace_id=$1")
                .bind(workspace_id)
                .fetch_all(&self.db.pool)
                .await?,
        )
    }

    /// Duplicate a chart in place (same dataset/viz/params/folder), owned by `new_owner`.
    pub async fn duplicate(
        &self,
        src_id: Uuid,
        new_owner: Option<Uuid>,
    ) -> Result<Option<Chart>, BiError> {
        let Some(src) = self.get(src_id).await? else {
            return Ok(None);
        };
        let existing = self.names_in_workspace(src.workspace_id).await?;
        let name = next_copy_name(&existing, &src.name);
        let sql = format!(
            "INSERT INTO charts (workspace_id, dataset_id, name, description, viz_type, params, created_by, folder_id) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8) RETURNING {COLS}"
        );
        let c = sqlx::query_as::<_, Chart>(&sql)
            .bind(src.workspace_id)
            .bind(src.dataset_id)
            .bind(&name)
            .bind(&src.description)
            .bind(&src.viz_type)
            .bind(&src.params)
            .bind(new_owner)
            .bind(src.folder_id)
            .fetch_one(&self.db.pool)
            .await
            .map_err(map_conflict_err)?;
        Ok(Some(c))
    }
}

/// First free "<base> (copy)" / "<base> (copy N)" not present in `existing`.
pub fn next_copy_name(existing: &[String], base: &str) -> String {
    let first = format!("{base} (copy)");
    if !existing.iter().any(|n| n == &first) {
        return first;
    }
    for n in 2.. {
        let cand = format!("{base} (copy {n})");
        if !existing.iter().any(|e| e == &cand) {
            return cand;
        }
    }
    unreachable!()
}
