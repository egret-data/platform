//! Postgres-backed workspace membership.

use egret_core::{Error, Result, UserId, WorkspaceId};
use egret_metastore::Db;
use uuid::Uuid;

use crate::model::{WorkspaceMember, WORKSPACE_ROLES};
use crate::workspaces::db_err;

#[derive(Clone)]
pub struct MemberRepo {
    db: Db,
}

impl MemberRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub async fn add(
        &self,
        ws: WorkspaceId,
        ptype: &str,
        pid: Uuid,
        role: &str,
        by: UserId,
    ) -> Result<()> {
        if ptype != "user" && ptype != "group" {
            return Err(Error::InvalidInput(format!("bad principal_type: {ptype}")));
        }
        if !WORKSPACE_ROLES.contains(&role) {
            return Err(Error::InvalidInput(format!(
                "unknown workspace role: {role}"
            )));
        }
        // One workspace role per member: replace any existing workspace grant for
        // this principal in this workspace, then insert the new role.
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        sqlx::query(
            "DELETE FROM access_grants \
             WHERE principal_type = $1 AND principal_id = $2 AND scope_type = 'workspace' AND scope_id = $3",
        )
        .bind(ptype)
        .bind(pid)
        .bind(ws.0)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        sqlx::query(
            "INSERT INTO access_grants \
               (principal_type, principal_id, role_id, scope_type, scope_id, granted_by) \
             SELECT $1, $2, r.id, 'workspace', $3, $4 FROM roles r \
             WHERE r.scope = 'workspace' AND r.name = $5 \
             ON CONFLICT DO NOTHING",
        )
        .bind(ptype)
        .bind(pid)
        .bind(ws.0)
        .bind(by.0)
        .bind(role)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }

    pub async fn remove(&self, ws: WorkspaceId, ptype: &str, pid: Uuid) -> Result<()> {
        sqlx::query(
            "DELETE FROM access_grants \
             WHERE principal_type = $1 AND principal_id = $2 AND scope_type = 'workspace' AND scope_id = $3",
        )
        .bind(ptype)
        .bind(pid)
        .bind(ws.0)
        .execute(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    pub async fn list(&self, ws: WorkspaceId) -> Result<Vec<WorkspaceMember>> {
        let rows = sqlx::query_as::<_, MemberRow>(
            "SELECT ag.principal_type, ag.principal_id, r.name AS role \
             FROM access_grants ag JOIN roles r ON r.id = ag.role_id \
             WHERE ag.scope_type = 'workspace' AND ag.scope_id = $1",
        )
        .bind(ws.0)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(Into::into).collect())
    }
}

#[derive(sqlx::FromRow)]
struct MemberRow {
    principal_type: String,
    principal_id: Uuid,
    role: String,
}

impl From<MemberRow> for WorkspaceMember {
    fn from(r: MemberRow) -> Self {
        WorkspaceMember {
            principal_type: r.principal_type,
            principal_id: r.principal_id,
            role: r.role,
        }
    }
}
