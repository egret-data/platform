//! Folder repository: per-type, flat folders. entity_type is 'chart'|'dashboard'|'dataset'.

use egret_metastore::Db;
use uuid::Uuid;

use crate::error::BiError;

#[derive(Clone)]
pub struct FolderRepo {
    db: Db,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Folder {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub entity_type: String,
    pub name: String,
    pub created_by: Option<Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

const COLS: &str = "id, workspace_id, entity_type, name, created_by, created_at";

fn map_conflict_err(e: sqlx::Error) -> BiError {
    if let sqlx::Error::Database(ref dbe) = e {
        if dbe.is_unique_violation() {
            return BiError::Conflict("a folder with that name already exists".into());
        }
    }
    BiError::Db(e)
}

impl FolderRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub async fn list(
        &self,
        workspace_id: Uuid,
        entity_type: &str,
    ) -> Result<Vec<Folder>, BiError> {
        let sql = format!(
            "SELECT {COLS} FROM analytics.folders WHERE workspace_id=$1 AND entity_type=$2 ORDER BY name"
        );
        Ok(sqlx::query_as::<_, Folder>(sqlx::AssertSqlSafe(sql))
            .bind(workspace_id)
            .bind(entity_type)
            .fetch_all(&self.db.pool)
            .await?)
    }

    pub async fn create(
        &self,
        workspace_id: Uuid,
        entity_type: &str,
        name: &str,
        created_by: Option<Uuid>,
    ) -> Result<Folder, BiError> {
        let sql = format!(
            "INSERT INTO analytics.folders (workspace_id, entity_type, name, created_by) \
             VALUES ($1,$2,$3,$4) RETURNING {COLS}"
        );
        sqlx::query_as::<_, Folder>(sqlx::AssertSqlSafe(sql))
            .bind(workspace_id)
            .bind(entity_type)
            .bind(name)
            .bind(created_by)
            .fetch_one(&self.db.pool)
            .await
            .map_err(map_conflict_err)
    }

    pub async fn rename(&self, id: Uuid, name: &str) -> Result<Option<Folder>, BiError> {
        let sql = format!("UPDATE analytics.folders SET name=$2 WHERE id=$1 RETURNING {COLS}");
        sqlx::query_as::<_, Folder>(sqlx::AssertSqlSafe(sql))
            .bind(id)
            .bind(name)
            .fetch_optional(&self.db.pool)
            .await
            .map_err(map_conflict_err)
    }

    pub async fn delete(&self, id: Uuid) -> Result<(), BiError> {
        sqlx::query("DELETE FROM analytics.folders WHERE id=$1")
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    /// (workspace_id, entity_type) of a folder, for the ownership/type guard. None if absent.
    pub async fn meta(&self, id: Uuid) -> Result<Option<(Uuid, String)>, BiError> {
        Ok(sqlx::query_as::<_, (Uuid, String)>(
            "SELECT workspace_id, entity_type FROM analytics.folders WHERE id=$1",
        )
        .bind(id)
        .fetch_optional(&self.db.pool)
        .await?)
    }
}
