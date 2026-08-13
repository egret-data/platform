//! Reusable SQL snippet repository (SQL Lab). Workspace-scoped content; all
//! reads and writes are filtered by `workspace_id` so a snippet id from one
//! workspace can't be updated/deleted through another (no cross-tenant IDOR).

use egret_core::UserId;
use egret_metastore::Db;
use uuid::Uuid;

use crate::error::BiError;

#[derive(Clone)]
pub struct SnippetRepo {
    db: Db,
}

#[derive(sqlx::FromRow)]
pub struct Snippet {
    pub id: Uuid,
    pub name: String,
    pub body: String,
    pub description: String,
    pub created_by: Option<Uuid>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

const COLS: &str = "id, name, body, description, created_by, updated_at";

impl SnippetRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub async fn list(&self, ws: Uuid) -> Result<Vec<Snippet>, BiError> {
        Ok(sqlx::query_as::<_, Snippet>(sqlx::AssertSqlSafe(format!(
            "SELECT {COLS} FROM analytics.sql_snippets WHERE workspace_id=$1 ORDER BY updated_at DESC"
        )))
        .bind(ws)
        .fetch_all(&self.db.pool)
        .await?)
    }

    pub async fn create(
        &self,
        ws: Uuid,
        name: &str,
        body: &str,
        description: &str,
        user: Option<UserId>,
    ) -> Result<Snippet, BiError> {
        Ok(sqlx::query_as::<_, Snippet>(sqlx::AssertSqlSafe(format!(
            "INSERT INTO analytics.sql_snippets (workspace_id, name, body, description, created_by) \
             VALUES ($1,$2,$3,$4,$5) RETURNING {COLS}"
        )))
        .bind(ws)
        .bind(name)
        .bind(body)
        .bind(description)
        .bind(user.map(|u| u.0))
        .fetch_one(&self.db.pool)
        .await?)
    }

    pub async fn update(
        &self,
        ws: Uuid,
        id: Uuid,
        name: &str,
        body: &str,
        description: &str,
    ) -> Result<Option<Snippet>, BiError> {
        Ok(sqlx::query_as::<_, Snippet>(sqlx::AssertSqlSafe(format!(
            "UPDATE analytics.sql_snippets SET name=$3, body=$4, description=$5, updated_at=now() \
             WHERE id=$2 AND workspace_id=$1 RETURNING {COLS}"
        )))
        .bind(ws)
        .bind(id)
        .bind(name)
        .bind(body)
        .bind(description)
        .fetch_optional(&self.db.pool)
        .await?)
    }

    pub async fn delete(&self, ws: Uuid, id: Uuid) -> Result<(), BiError> {
        sqlx::query("DELETE FROM analytics.sql_snippets WHERE id=$1 AND workspace_id=$2")
            .bind(id)
            .bind(ws)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }
}
