//! RBAC: most-permissive union of permissions across a principal's roles (direct + group).

use std::collections::HashSet;

use egret_core::{Error, Principal, Result, UserId, WorkspaceContext, WorkspaceId};
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

    /// Union of platform permissions over the user's platform roles (direct + group).
    pub async fn platform_permissions(&self, user_id: UserId) -> Result<HashSet<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT DISTINCT rp.permission \
             FROM platform_role_assignments pra \
             JOIN roles r ON r.id = pra.role_id \
             JOIN role_permissions rp ON rp.role_id = r.id \
             WHERE (pra.principal_type = 'user'  AND pra.principal_id = $1) \
                OR (pra.principal_type = 'group' AND pra.principal_id IN \
                       (SELECT group_id FROM user_groups WHERE user_id = $1))",
        )
        .bind(user_id.0)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(|(p,)| p).collect())
    }

    /// Union of workspace permissions over the principal's workspace roles for `ws` (direct + group).
    pub async fn workspace_permissions(
        &self,
        user_id: UserId,
        ws: WorkspaceId,
    ) -> Result<HashSet<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT DISTINCT rp.permission \
             FROM workspace_members wm \
             JOIN roles r ON r.scope = 'workspace' AND r.name = wm.role \
             JOIN role_permissions rp ON rp.role_id = r.id \
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
        Ok(rows.into_iter().map(|(p,)| p).collect())
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

    pub async fn require_platform(&self, p: &Principal, perm: &str) -> Result<()> {
        if self.platform_permissions(p.user_id).await?.contains(perm) {
            Ok(())
        } else {
            Err(Error::Forbidden)
        }
    }

    pub async fn require_workspace(&self, ctx: &WorkspaceContext, perm: &str) -> Result<()> {
        if ctx.is_platform_admin() {
            return Ok(());
        }
        if self
            .workspace_permissions(ctx.principal.user_id, ctx.workspace_id)
            .await?
            .contains(perm)
        {
            Ok(())
        } else {
            Err(Error::Forbidden)
        }
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
