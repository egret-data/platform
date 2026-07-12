//! Effective-access resolver (AX-3): for a principal, compute what they can do,
//! where, and why — enumerable from the data (Cedar can't enumerate). Read-only.

use egret_metastore::Db;
use serde::Serialize;
use uuid::Uuid;

use crate::roles::db_err; // reuse the existing error mapper

#[derive(Debug, Serialize)]
pub struct EffectiveAccess {
    pub principal: PrincipalInfo,
    pub platform_roles: Vec<RoleSource>,
    pub permissions: Vec<EffectivePermission>,
    pub workspaces: Vec<WorkspaceMembership>,
    pub objects: Vec<ObjectGrant>,
    pub denies: Vec<DenyEntry>,
}

#[derive(Debug, Serialize)]
pub struct PrincipalInfo {
    pub r#type: String,
    pub id: String,
    pub name: Option<String>,
    pub is_platform_admin: bool,
}

#[derive(Debug, Serialize, Clone)]
pub struct RoleSource {
    pub name: String,
    pub source: String, // "direct" | "group:{groupName}"
}

#[derive(Debug, Serialize)]
pub struct PermVia {
    pub role: String,
    pub inherited_from: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct EffectivePermission {
    pub key: String,
    pub via: Vec<PermVia>,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceMembership {
    pub workspace_id: String,
    pub workspace_slug: String,
    pub workspace_name: String,
    pub role: String,
    pub source: String,
}

#[derive(Debug, Serialize)]
pub struct ObjectGrant {
    pub kind: String,
    pub id: String,
    pub name: Option<String>,
    pub workspace_slug: Option<String>,
    pub role: String,
    pub effect: String,
    pub source: String,
}

#[derive(Debug, Serialize)]
pub struct DenyEntry {
    pub scope: String,
    pub target: String,
    pub role: String,
    pub source: String,
}

#[derive(Clone)]
pub struct AccessResolver {
    db: Db,
}

impl AccessResolver {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Every grant feeding this principal, with a `source` label. For a user, this
    /// is their own grants plus grants held by any group they belong to; for a
    /// group, only its own grants (egret groups don't nest).
    #[allow(dead_code)] // removed in Task 3
    async fn grant_rows(&self, ptype: &str, pid: Uuid) -> Result<Vec<GrantRow>, egret_core::Error> {
        let sql = "
            SELECT ag.scope_type, ag.scope_id, ag.object_type, ag.effect,
                   r.name AS role, r.id AS role_id, r.scope AS role_scope,
                   'direct' AS source
              FROM access_grants ag JOIN roles r ON r.id = ag.role_id
             WHERE ag.principal_type = $1 AND ag.principal_id = $2
            UNION ALL
            SELECT ag.scope_type, ag.scope_id, ag.object_type, ag.effect,
                   r.name AS role, r.id AS role_id, r.scope AS role_scope,
                   'group:' || g.name AS source
              FROM access_grants ag
              JOIN roles r  ON r.id = ag.role_id
              JOIN groups g ON g.id = ag.principal_id AND ag.principal_type = 'group'
             WHERE $1 = 'user'
               AND ag.principal_id IN (SELECT group_id FROM user_groups WHERE user_id = $2)
        ";
        sqlx::query_as::<_, GrantRow>(sql)
            .bind(ptype)
            .bind(pid)
            .fetch_all(&self.db.pool)
            .await
            .map_err(db_err)
    }

    #[allow(dead_code)] // removed in Task 3
    async fn principal_info(
        &self,
        ptype: &str,
        pid: Uuid,
        grants: &[GrantRow],
    ) -> Result<PrincipalInfo, egret_core::Error> {
        let name: Option<String> = match ptype {
            "user" => sqlx::query_scalar("SELECT email::text FROM users WHERE id = $1"),
            "group" => sqlx::query_scalar("SELECT name FROM groups WHERE id = $1"),
            _ => {
                return Err(egret_core::Error::InvalidInput(format!(
                    "bad principal_type: {ptype}"
                )))
            }
        }
        .bind(pid)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        let is_platform_admin = grants.iter().any(|g| {
            g.scope_type == "platform" && g.role == "platform_admin" && g.effect == "allow"
        });
        Ok(PrincipalInfo {
            r#type: ptype.to_string(),
            id: pid.to_string(),
            name,
            is_platform_admin,
        })
    }

    #[allow(dead_code)] // removed in Task 3
    fn platform_roles(grants: &[GrantRow]) -> Vec<RoleSource> {
        grants
            .iter()
            .filter(|g| g.scope_type == "platform" && g.effect == "allow")
            .map(|g| RoleSource {
                name: g.role.clone(),
                source: g.source.clone(),
            })
            .collect()
    }
}

#[derive(sqlx::FromRow, Clone)]
#[allow(dead_code)]
struct GrantRow {
    scope_type: String,
    scope_id: Option<Uuid>,
    object_type: Option<String>,
    effect: String,
    role: String,
    role_id: Uuid,
    role_scope: String,
    source: String,
}
