//! RBAC: most-permissive union of permissions across a principal's roles (direct + group).

use egret_core::{Error, Result, UserId, WorkspaceId};
use egret_metastore::Db;

use crate::model::PLATFORM_ROLES;
use crate::workspaces::db_err;

#[derive(Clone)]
pub struct Authorizer {
    db: Db,
}

impl Authorizer {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// The principal's workspace role names for `ws` (direct + group), for display precedence.
    pub async fn workspace_roles_for(
        &self,
        user_id: UserId,
        ws: WorkspaceId,
    ) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT DISTINCT wm.role FROM workspace_members wm \
             WHERE wm.workspace_id = $1 \
               AND ( (wm.principal_type = 'user'  AND wm.principal_id = $2) \
                  OR (wm.principal_type = 'group' AND wm.principal_id IN \
                         (SELECT group_id FROM user_groups WHERE user_id = $2)) )",
        )
        .bind(ws.0)
        .bind(user_id.0)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(|(r,)| r).collect())
    }

    /// Replace the user's platform role assignments with the supplied (validated) role names.
    pub async fn replace_platform_roles(&self, user_id: UserId, roles: &[String]) -> Result<()> {
        for r in roles {
            if !PLATFORM_ROLES.contains(&r.as_str()) {
                return Err(Error::InvalidInput(format!("unknown platform role: {r}")));
            }
        }
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        sqlx::query(
            "DELETE FROM platform_role_assignments WHERE principal_type = 'user' AND principal_id = $1",
        )
        .bind(user_id.0)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        for r in roles {
            sqlx::query(
                "INSERT INTO platform_role_assignments (principal_type, principal_id, role_id) \
                 SELECT 'user', $1, id FROM roles WHERE scope = 'platform' AND name = $2",
            )
            .bind(user_id.0)
            .bind(r)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }
}
