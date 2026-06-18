//! Workspace grants for databases + grant-aware listing.

use egret_core::UserId;
use uuid::Uuid;

use crate::error::DatabaseError;
use crate::model::Database;
use crate::repo::DatabaseRepo;

impl DatabaseRepo {
    /// Grant a database to a workspace (idempotent).
    pub async fn grant(
        &self,
        workspace_id: Uuid,
        database_id: Uuid,
        granted_by: Option<UserId>,
    ) -> Result<(), DatabaseError> {
        sqlx::query(
            "INSERT INTO workspace_database_grant (workspace_id, database_id, granted_by) \
             VALUES ($1, $2, $3) ON CONFLICT (workspace_id, database_id) DO NOTHING",
        )
        .bind(workspace_id)
        .bind(database_id)
        .bind(granted_by.map(|u| u.0))
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Revoke a database from a workspace (idempotent).
    pub async fn revoke(&self, workspace_id: Uuid, database_id: Uuid) -> Result<(), DatabaseError> {
        sqlx::query(
            "DELETE FROM workspace_database_grant WHERE workspace_id = $1 AND database_id = $2",
        )
        .bind(workspace_id)
        .bind(database_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Databases granted to a workspace (ordered by name).
    pub async fn list_for_workspace(
        &self,
        workspace_id: Uuid,
    ) -> Result<Vec<Database>, DatabaseError> {
        let rows = sqlx::query_as::<_, crate::repo::Row>(
            "SELECT d.id, d.name, d.engine, d.host, d.port, d.options, d.secret_id, d.status \
             FROM databases d \
             JOIN workspace_database_grant g ON g.database_id = d.id \
             WHERE g.workspace_id = $1 ORDER BY d.name",
        )
        .bind(workspace_id)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter()
            .map(crate::repo::Row::into_database)
            .collect()
    }
}
