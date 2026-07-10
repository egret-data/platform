//! Read API over the unified `access_grants` table.

use egret_core::ids::{UserId, WorkspaceId};
use egret_core::{Error, Result};
use egret_metastore::Db;

/// A single grant relevant to a decision.
#[derive(Debug, Clone)]
pub struct Grant {
    pub role_id: uuid::Uuid,
    pub scope_type: String, // 'platform' | 'workspace' | 'object'
    pub scope_id: Option<uuid::Uuid>,
    /// For object-scoped grants: the object kind ("dashboard" | "folder" | …)
    /// that namespaces the Cedar `Object` entity id. `None` for platform/workspace
    /// grants. Populated from SQL in Task 3.
    pub object_type: Option<String>,
}

#[derive(Clone)]
pub struct AccessGrantStore {
    db: Db,
}

impl AccessGrantStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Every grant held by the user directly or via a group, at platform scope
    /// or for the given workspace. (Object scope arrives in B2.)
    pub async fn grants_for(&self, user_id: UserId, ws: Option<WorkspaceId>) -> Result<Vec<Grant>> {
        let rows: Vec<(uuid::Uuid, String, Option<uuid::Uuid>)> = sqlx::query_as(
            "SELECT ag.role_id, ag.scope_type, ag.scope_id \
             FROM access_grants ag \
             WHERE ( (ag.principal_type = 'user'  AND ag.principal_id = $1) \
                  OR (ag.principal_type = 'group' AND ag.principal_id IN \
                        (SELECT group_id FROM user_groups WHERE user_id = $1)) ) \
               AND ( ag.scope_type = 'platform' \
                  OR (ag.scope_type = 'workspace' AND ag.scope_id = $2) )",
        )
        .bind(user_id.0)
        .bind(ws.map(|w| w.0))
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows
            .into_iter()
            .map(|(role_id, scope_type, scope_id)| Grant {
                role_id,
                scope_type,
                scope_id,
                object_type: None,
            })
            .collect())
    }
}

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::anyhow!(e))
}
