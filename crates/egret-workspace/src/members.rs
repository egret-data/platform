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
        sqlx::query(
            "INSERT INTO workspace_members (workspace_id, principal_type, principal_id, role, added_by) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (workspace_id, principal_type, principal_id) DO UPDATE SET role = EXCLUDED.role",
        )
        .bind(ws.0)
        .bind(ptype)
        .bind(pid)
        .bind(role)
        .bind(by.0)
        .execute(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    pub async fn remove(&self, ws: WorkspaceId, ptype: &str, pid: Uuid) -> Result<()> {
        sqlx::query(
            "DELETE FROM workspace_members WHERE workspace_id = $1 AND principal_type = $2 AND principal_id = $3",
        )
        .bind(ws.0)
        .bind(ptype)
        .bind(pid)
        .execute(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    pub async fn list(&self, ws: WorkspaceId) -> Result<Vec<WorkspaceMember>> {
        let rows = sqlx::query_as::<_, MemberRow>(
            "SELECT principal_type, principal_id, role FROM workspace_members WHERE workspace_id = $1",
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
