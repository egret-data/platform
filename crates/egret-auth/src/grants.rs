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
    /// 'allow' | 'deny'. Deny links the `role-deny:{id}` forbid template.
    pub effect: String,
}

/// (role_id, scope_type, scope_id, object_type, effect) as read from SQL.
type GrantRow = (
    uuid::Uuid,
    String,
    Option<uuid::Uuid>,
    Option<String>,
    String,
);

#[derive(Clone)]
pub struct AccessGrantStore {
    db: Db,
}

impl AccessGrantStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Every grant held by the user directly or via a group, at platform scope,
    /// for the given workspace, or for any of the given object scopes.
    ///
    /// `object_scope_ids` is the set of object ids whose grants may reach the
    /// resource under decision — for an object resource that is the object's own
    /// id plus its folder id (so a folder grant cascades to items inside it).
    /// Pass `&[]` when no object scope is relevant (platform/workspace resources).
    pub async fn grants_for(
        &self,
        user_id: UserId,
        ws: Option<WorkspaceId>,
        object_scope_ids: &[uuid::Uuid],
    ) -> Result<Vec<Grant>> {
        let rows: Vec<GrantRow> = sqlx::query_as(
            "SELECT ag.role_id, ag.scope_type, ag.scope_id, ag.object_type, ag.effect \
             FROM access_grants ag \
             WHERE ( (ag.principal_type = 'user'  AND ag.principal_id = $1) \
                  OR (ag.principal_type = 'group' AND ag.principal_id IN \
                        (SELECT group_id FROM user_groups WHERE user_id = $1)) ) \
               AND ( ag.scope_type = 'platform' \
                  OR (ag.scope_type = 'workspace' AND ag.scope_id = $2) \
                  OR (ag.scope_type = 'object' AND ag.scope_id = ANY($3)) ) \
               AND (ag.expires_at IS NULL OR ag.expires_at > now())",
        )
        .bind(user_id.0)
        .bind(ws.map(|w| w.0))
        .bind(object_scope_ids)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows
            .into_iter()
            .map(
                |(role_id, scope_type, scope_id, object_type, effect)| Grant {
                    role_id,
                    scope_type,
                    scope_id,
                    object_type,
                    effect,
                },
            )
            .collect())
    }
}

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::anyhow!(e))
}
