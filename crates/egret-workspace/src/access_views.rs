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

use egret_core::{Result, WorkspaceId};
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

/// One OBJECT grant whose object lives in a given workspace, enriched with the
/// object's display name, the granted role name, the principal's display name,
/// and the grant `effect` ('allow' | 'deny').
#[derive(Debug, Clone)]
pub struct ObjectGrantRollup {
    /// Object type: 'chart' | 'dashboard' | 'notebook' | 'dataset' | 'saved_query' | 'folder'.
    pub kind: String,
    pub object_id: Uuid,
    pub object_name: Option<String>,
    /// Principal type: 'user' | 'group' | 'service'.
    pub principal_type: String,
    pub principal_id: Uuid,
    pub principal_name: Option<String>,
    /// The granted role's name (e.g. 'ws_viewer').
    pub role: String,
    /// 'allow' | 'deny' (legacy NULL rows coalesced to 'allow').
    pub effect: String,
    /// Optional expiry (P2a). `None` = never expires. This is the MANAGEMENT
    /// roll-up, so an expired grant is still listed here (no expiry filter) —
    /// only the read-time admission predicates exclude expired grants.
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
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
        //
        // `data_api_invoker` is excluded HERE, in the query, rather than by the
        // route handler that currently is this method's only caller. This is the
        // workspace-scope role catalog a member picker offers, and
        // `data_api_invoker` (seeded by migration 0124, egret-dataapi — literal
        // string here to avoid a cross-crate dependency, same convention as
        // `egret-bi`'s `kind_perms`) exists to be granted OBJECT-scoped to one
        // service account for one endpoint. Offered as a workspace MEMBERSHIP
        // role it would mean "invoke every endpoint in this workspace" through
        // Cedar's workspace→object cascade — the same over-grant a `ws_admin`
        // membership now carries. Filtering in the query means a future SECOND
        // consumer of this method inherits the exclusion automatically, instead
        // of needing to remember to filter it out itself.
        let rows = sqlx::query_as::<_, RolePermRow>(
            "SELECT r.id, r.name, r.description, r.parent_role_id, rp.permission \
             FROM roles r \
             LEFT JOIN role_permissions rp ON rp.role_id = r.id \
             WHERE r.scope = 'workspace' AND r.name <> 'data_api_invoker' \
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

    /// Every OBJECT grant whose object lives in `workspace_id`, enriched with the
    /// object's name, the granted role name, the principal's display name and the
    /// grant effect.
    ///
    /// Object → workspace resolution reuses the same object tables the object-scope
    /// admission path uses (`Authorizer::has_object_grant_in_workspace`): a
    /// `UNION ALL` over the six object tables (each carries `workspace_id` + a
    /// `name` column), filtered to this workspace, then joined back to
    /// `access_grants` on `(object_type, scope_id)`. Restricting the object set to
    /// this workspace in SQL is the cross-workspace IDOR guard — a grant on an
    /// object that lives in another workspace can never join through, so it is
    /// never returned.
    pub async fn list_object_grants(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<Vec<ObjectGrantRollup>> {
        // Object tables — all in the `analytics` schema, each with `workspace_id`
        // and a `name` display column (confirmed against migrations 0012/0014/0015/
        // 0021 + 0010 saved_queries, moved to `analytics` in 0067):
        //   analytics.charts, analytics.dashboards, analytics.datasets,
        //   analytics.folders, analytics.saved_queries,
        //   analytics.apps (0161; trashed apps included, as their shares still exist).
        let rows = sqlx::query_as::<_, ObjectGrantRow>(
            "WITH ws_objects (kind, id, name) AS ( \
                 SELECT 'chart',       id, name FROM analytics.charts        WHERE workspace_id = $1 \
                 UNION ALL SELECT 'dashboard',   id, name FROM analytics.dashboards    WHERE workspace_id = $1 \
                 UNION ALL SELECT 'dataset',     id, name FROM analytics.datasets      WHERE workspace_id = $1 \
                 UNION ALL SELECT 'folder',      id, name FROM analytics.folders       WHERE workspace_id = $1 \
                 UNION ALL SELECT 'saved_query', id, name FROM analytics.saved_queries WHERE workspace_id = $1 \
                 UNION ALL SELECT 'app',         id, name FROM analytics.apps          WHERE workspace_id = $1 \
             ) \
             SELECT o.kind, o.id AS object_id, o.name AS object_name, \
                    ag.principal_type, ag.principal_id, \
                    COALESCE(u.email::text, g.name) AS principal_name, \
                    r.name AS role, COALESCE(ag.effect, 'allow') AS effect, \
                    ag.expires_at \
             FROM access_grants ag \
             JOIN ws_objects o ON o.kind = ag.object_type AND o.id = ag.scope_id \
             JOIN roles r ON r.id = ag.role_id \
             LEFT JOIN users  u ON ag.principal_type = 'user'  AND u.id = ag.principal_id \
             LEFT JOIN groups g ON ag.principal_type = 'group' AND g.id = ag.principal_id \
             WHERE ag.scope_type = 'object' \
             ORDER BY o.kind, o.name, r.name",
        )
        .bind(workspace_id.0)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;

        Ok(rows
            .into_iter()
            .map(|r| ObjectGrantRollup {
                kind: r.kind,
                object_id: r.object_id,
                object_name: r.object_name,
                principal_type: r.principal_type,
                principal_id: r.principal_id,
                principal_name: r.principal_name,
                role: r.role,
                effect: r.effect,
                expires_at: r.expires_at,
            })
            .collect())
    }
}

#[derive(sqlx::FromRow)]
struct ObjectGrantRow {
    kind: String,
    object_id: Uuid,
    object_name: Option<String>,
    principal_type: String,
    principal_id: Uuid,
    principal_name: Option<String>,
    role: String,
    effect: String,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(sqlx::FromRow)]
struct RolePermRow {
    id: Uuid,
    name: String,
    description: String,
    parent_role_id: Option<Uuid>,
    permission: Option<String>,
}
