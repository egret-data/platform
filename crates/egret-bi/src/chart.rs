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
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

const COLS: &str =
    "id, workspace_id, dataset_id, name, description, viz_type, params, created_by, updated_at";

/// Map a unique-violation on (workspace_id, name) to a Conflict; pass everything else through.
fn map_create_err(e: sqlx::Error) -> BiError {
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
            .map_err(map_create_err)
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

    pub async fn update(
        &self,
        id: Uuid,
        name: &str,
        description: &str,
        viz_type: &str,
        params: &serde_json::Value,
    ) -> Result<(), BiError> {
        sqlx::query(
            "UPDATE charts SET name=$2, description=$3, viz_type=$4, params=$5, updated_at=now() WHERE id=$1",
        )
        .bind(id)
        .bind(name)
        .bind(description)
        .bind(viz_type)
        .bind(params)
        .execute(&self.db.pool)
        .await
        .map_err(map_create_err)?;
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
}
