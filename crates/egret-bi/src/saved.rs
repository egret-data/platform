//! Saved-query repository.

use egret_core::UserId;
use egret_metastore::Db;
use uuid::Uuid;

use crate::error::BiError;

#[derive(Clone)]
pub struct SavedQueryRepo {
    db: Db,
}

#[derive(sqlx::FromRow)]
pub struct SavedQuery {
    pub id: Uuid,
    pub database_id: Option<Uuid>,
    pub name: String,
    pub sql: String,
    pub description: String,
    pub created_by: Option<Uuid>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

const COLS: &str = "id, database_id, name, sql, description, created_by, updated_at";

impl SavedQueryRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub async fn create(
        &self,
        ws: Uuid,
        db_id: Option<Uuid>,
        name: &str,
        sql: &str,
        description: &str,
        user: Option<UserId>,
    ) -> Result<SavedQuery, BiError> {
        let row = sqlx::query_as::<_, SavedQuery>(&format!(
            "INSERT INTO saved_queries (workspace_id, database_id, name, sql, description, created_by) \
             VALUES ($1,$2,$3,$4,$5,$6) RETURNING {COLS}"
        ))
        .bind(ws)
        .bind(db_id)
        .bind(name)
        .bind(sql)
        .bind(description)
        .bind(user.map(|u| u.0))
        .fetch_one(&self.db.pool)
        .await?;
        Ok(row)
    }

    pub async fn list(&self, ws: Uuid) -> Result<Vec<SavedQuery>, BiError> {
        let rows = sqlx::query_as::<_, SavedQuery>(&format!(
            "SELECT {COLS} FROM saved_queries WHERE workspace_id=$1 ORDER BY updated_at DESC"
        ))
        .bind(ws)
        .fetch_all(&self.db.pool)
        .await?;
        Ok(rows)
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<SavedQuery>, BiError> {
        let row = sqlx::query_as::<_, SavedQuery>(&format!(
            "SELECT {COLS} FROM saved_queries WHERE id=$1"
        ))
        .bind(id)
        .fetch_optional(&self.db.pool)
        .await?;
        Ok(row)
    }

    pub async fn update(
        &self,
        id: Uuid,
        name: &str,
        sql: &str,
        description: &str,
        db_id: Option<Uuid>,
    ) -> Result<SavedQuery, BiError> {
        let row = sqlx::query_as::<_, SavedQuery>(&format!(
            "UPDATE saved_queries SET name=$2, sql=$3, description=$4, database_id=$5, updated_at=now() WHERE id=$1 RETURNING {COLS}"))
            .bind(id).bind(name).bind(sql).bind(description).bind(db_id)
            .fetch_optional(&self.db.pool).await?
            .ok_or_else(|| BiError::NotFound(id.to_string()))?;
        Ok(row)
    }

    pub async fn delete(&self, id: Uuid) -> Result<(), BiError> {
        sqlx::query("DELETE FROM saved_queries WHERE id=$1")
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    /// The owner's id (for the owner-or-admin edit/delete gate).
    pub async fn owner(&self, id: Uuid) -> Result<Option<Uuid>, BiError> {
        Ok(
            sqlx::query_scalar("SELECT created_by FROM saved_queries WHERE id=$1")
                .bind(id)
                .fetch_optional(&self.db.pool)
                .await?
                .flatten(),
        )
    }
}
