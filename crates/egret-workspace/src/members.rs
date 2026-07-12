//! Postgres-backed workspace membership.

use egret_core::{Error, Result, UserId, WorkspaceId};
use egret_metastore::Db;
use uuid::Uuid;

use crate::model::WorkspaceMember;
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
        let role_ok: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM roles WHERE scope = 'workspace' AND name = $1)",
        )
        .bind(role)
        .fetch_one(&self.db.pool)
        .await
        .map_err(db_err)?;
        if !role_ok {
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
            "SELECT ag.principal_type, ag.principal_id, r.name AS role, \
                    COALESCE(u.email::text, g.name) AS principal_name, \
                    gb.email::text AS granted_by_name, \
                    ag.created_at AS added_at, \
                    EXISTS ( \
                      SELECT 1 FROM access_grants pa JOIN roles pr ON pr.id = pa.role_id \
                      WHERE pa.principal_type = ag.principal_type AND pa.principal_id = ag.principal_id \
                        AND pa.scope_type = 'platform' AND pr.name = 'platform_admin' \
                    ) AS is_platform_admin \
             FROM access_grants ag \
             JOIN roles r ON r.id = ag.role_id \
             LEFT JOIN users  u  ON ag.principal_type = 'user'  AND u.id = ag.principal_id \
             LEFT JOIN groups g  ON ag.principal_type = 'group' AND g.id = ag.principal_id \
             LEFT JOIN users  gb ON gb.id = ag.granted_by \
             WHERE ag.scope_type = 'workspace' AND ag.scope_id = $1 \
             ORDER BY ag.created_at",
        )
        .bind(ws.0)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// Number of members holding `ws_admin` in the workspace.
    pub async fn count_ws_admins(&self, ws: WorkspaceId) -> Result<i64> {
        sqlx::query_scalar(
            "SELECT count(*) FROM access_grants ag JOIN roles r ON r.id = ag.role_id \
             WHERE ag.scope_type = 'workspace' AND ag.scope_id = $1 AND r.name = 'ws_admin'",
        )
        .bind(ws.0)
        .fetch_one(&self.db.pool)
        .await
        .map_err(db_err)
    }

    /// The workspace role a principal currently holds here, if any.
    pub async fn role_of(&self, ws: WorkspaceId, ptype: &str, pid: Uuid) -> Result<Option<String>> {
        sqlx::query_scalar(
            "SELECT r.name FROM access_grants ag JOIN roles r ON r.id = ag.role_id \
             WHERE ag.scope_type = 'workspace' AND ag.scope_id = $1 \
               AND ag.principal_type = $2 AND ag.principal_id = $3",
        )
        .bind(ws.0)
        .bind(ptype)
        .bind(pid)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)
    }
}

#[derive(sqlx::FromRow)]
struct MemberRow {
    principal_type: String,
    principal_id: Uuid,
    role: String,
    principal_name: Option<String>,
    granted_by_name: Option<String>,
    added_at: chrono::DateTime<chrono::Utc>,
    is_platform_admin: bool,
}

impl From<MemberRow> for WorkspaceMember {
    fn from(r: MemberRow) -> Self {
        WorkspaceMember {
            principal_type: r.principal_type,
            principal_id: r.principal_id,
            role: r.role,
            principal_name: r.principal_name,
            granted_by_name: r.granted_by_name,
            added_at: r.added_at,
            is_platform_admin: r.is_platform_admin,
        }
    }
}
