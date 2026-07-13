//! Read-only workspace-scope role catalog.
//!
//! A workspace admin needs to SEE what each workspace role grants without the
//! platform-level `role:read` permission that gates the control-plane access
//! APIs. This repo answers exactly that question, workspace-scoped: for every
//! `scope = 'workspace'` role, its direct permission keys.
//!
//! A role's direct permissions live in the `role_permissions` join table
//! (`role_id`, `permission`) — the same store the control-plane
//! `set_role_permissions`/`list_permissions` handlers read (via
//! `egret_auth::roles::RoleRepo`). Each permission key is enriched here with its
//! human description + group label from the code-defined catalog
//! (`egret_core::rbac::PERMISSIONS`), mirroring how the CP `list_permissions`
//! handler folds catalog metadata over the stored keys.

use std::collections::HashMap;

use egret_core::Result;
use egret_metastore::Db;
use uuid::Uuid;

use crate::workspaces::db_err;

#[derive(Clone)]
pub struct AccessViewsRepo {
    db: Db,
}

/// A workspace-scope role with its direct (own) permissions.
#[derive(Debug, Clone)]
pub struct WsRole {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    /// The role this one inherits from (same scope), if any.
    pub parent_role_id: Option<Uuid>,
    pub permissions: Vec<WsPerm>,
}

/// A single permission key, enriched from the code-defined catalog.
#[derive(Debug, Clone)]
pub struct WsPerm {
    pub key: String,
    pub description: String,
    pub group: String,
}

impl AccessViewsRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Every workspace-scope role with its direct permission keys, roles ordered
    /// by name and permissions by key. Each permission carries the catalog
    /// description + group (empty strings if the key is not in the catalog —
    /// which shouldn't happen, since `role_permissions` references catalog keys).
    pub async fn list_workspace_roles_with_perms(&self) -> Result<Vec<WsRole>> {
        // One flat scan: every workspace role, LEFT-joined to its permissions so
        // roles with no permissions still appear. Folded into `WsRole`s below.
        let rows = sqlx::query_as::<_, RolePermRow>(
            "SELECT r.id, r.name, r.description, r.parent_role_id, rp.permission \
             FROM roles r \
             LEFT JOIN role_permissions rp ON rp.role_id = r.id \
             WHERE r.scope = 'workspace' \
             ORDER BY r.name, rp.permission",
        )
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;

        // Catalog lookup: key -> (description, group).
        let catalog: HashMap<&'static str, &'static egret_core::rbac::Permission> =
            egret_core::rbac::PERMISSIONS
                .iter()
                .map(|p| (p.key, p))
                .collect();

        let mut out: Vec<WsRole> = Vec::new();
        for row in rows {
            // Rows are grouped by role (ordered by name); extend the current role
            // or start a new one.
            if out.last().map(|r| r.id) != Some(row.id) {
                out.push(WsRole {
                    id: row.id,
                    name: row.name,
                    description: row.description,
                    parent_role_id: row.parent_role_id,
                    permissions: Vec::new(),
                });
            }
            if let Some(key) = row.permission {
                let (description, group) = catalog
                    .get(key.as_str())
                    .map(|p| (p.description.to_string(), p.group.to_string()))
                    .unwrap_or_default();
                out.last_mut().unwrap().permissions.push(WsPerm {
                    key,
                    description,
                    group,
                });
            }
        }
        Ok(out)
    }
}

#[derive(sqlx::FromRow)]
struct RolePermRow {
    id: Uuid,
    name: String,
    description: String,
    parent_role_id: Option<Uuid>,
    permission: Option<String>,
}
