//! Postgres-backed store for platform RBAC roles, their permissions, and
//! principal (user/group) assignments.

use crate::events::RolesChanged;
use egret_core::Result;
use egret_events::EventBus;
use egret_metastore::Db;
use uuid::Uuid;

/// A stored role row (`roles` table).
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct RoleRow {
    pub id: Uuid,
    pub scope: String,
    pub name: String,
    pub description: String,
    pub parent_role_id: Option<Uuid>,
}

#[derive(Clone)]
pub struct RoleRepo {
    db: Db,
    bus: EventBus,
}

/// Fixed advisory-lock key serializing role-graph parent mutations (`set_parent`),
/// so concurrent parent changes can't write-skew into a cycle. Arbitrary constant.
const ROLE_GRAPH_LOCK_KEY: i64 = 0x_E6E7_C0DE_6A11;

/// Names of roles seeded by migration 0003 (non-deletable; name/scope immutable).
pub const SEEDED_ROLES: &[&str] = &[
    "platform_admin",
    "data_steward",
    "ws_admin",
    "ws_editor",
    "ws_viewer",
];

/// Whether `name` is one of the roles seeded by migration 0003.
pub fn is_seeded(name: &str) -> bool {
    SEEDED_ROLES.contains(&name)
}

impl RoleRepo {
    pub fn new(db: Db, bus: EventBus) -> Self {
        Self { db, bus }
    }

    /// List all roles, ordered by scope then name.
    pub async fn list(&self) -> Result<Vec<RoleRow>> {
        sqlx::query_as::<_, RoleRow>(
            "SELECT id, scope, name, description, parent_role_id FROM roles ORDER BY scope, name",
        )
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)
    }

    /// Fetch a single role by id.
    pub async fn get(&self, id: Uuid) -> Result<Option<RoleRow>> {
        sqlx::query_as::<_, RoleRow>(
            "SELECT id, scope, name, description, parent_role_id FROM roles WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)
    }

    /// Permissions granted by a role, ordered by permission name.
    pub async fn permissions_of(&self, id: Uuid) -> Result<Vec<String>> {
        sqlx::query_scalar(
            "SELECT permission FROM role_permissions WHERE role_id = $1 ORDER BY permission",
        )
        .bind(id)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)
    }

    /// Principals (type, id) a role is directly assigned to.
    pub async fn assignees_of(&self, id: Uuid) -> Result<Vec<(String, Uuid)>> {
        sqlx::query_as(
            "SELECT principal_type, principal_id FROM access_grants \
             WHERE role_id = $1 AND scope_type = 'platform'",
        )
        .bind(id)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)
    }

    /// A role's assignees with display names: users by email, groups by name.
    pub async fn assignees_named(&self, role_id: Uuid) -> Result<Vec<(String, String)>> {
        // returns (kind, name) where kind is "user" | "group"
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT 'user' AS kind, u.email::text AS name \
               FROM access_grants pra JOIN users u ON u.id = pra.principal_id \
              WHERE pra.role_id = $1 AND pra.principal_type = 'user' AND pra.scope_type = 'platform' \
             UNION ALL \
             SELECT 'group' AS kind, g.name \
               FROM access_grants pra JOIN groups g ON g.id = pra.principal_id \
              WHERE pra.role_id = $1 AND pra.principal_type = 'group' AND pra.scope_type = 'platform' \
             ORDER BY name",
        )
        .bind(role_id)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows)
    }

    /// Roles directly assigned to a principal (does not expand group membership).
    pub async fn roles_of(&self, ptype: &str, pid: Uuid) -> Result<Vec<RoleRow>> {
        sqlx::query_as::<_, RoleRow>(
            "SELECT r.id, r.scope, r.name, r.description, r.parent_role_id FROM access_grants pra \
             JOIN roles r ON r.id = pra.role_id \
             WHERE pra.principal_type = $1 AND pra.principal_id = $2 AND pra.scope_type = 'platform' \
             ORDER BY r.name",
        )
        .bind(ptype)
        .bind(pid)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)
    }

    /// Every granted `(permission, role name)` pair across all roles — the raw
    /// join `GET /permissions` groups in Rust to attach each catalog
    /// permission's granting role names.
    pub async fn permission_role_pairs(&self) -> Result<Vec<(String, String)>> {
        sqlx::query_as(
            "SELECT rp.permission, r.name FROM role_permissions rp \
             JOIN roles r ON r.id = rp.role_id",
        )
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)
    }

    /// Count of principal assignments to the `platform_admin` role, used to
    /// guard against removing the last platform admin.
    pub async fn count_platform_admins(&self) -> Result<i64> {
        sqlx::query_scalar(
            "SELECT count(*) FROM access_grants pra \
             JOIN roles r ON r.id = pra.role_id \
             WHERE r.name = 'platform_admin' AND pra.scope_type = 'platform'",
        )
        .fetch_one(&self.db.pool)
        .await
        .map_err(db_err)
    }

    /// Create a new role; returns its id.
    /// Errors with `Conflict` if a role with the same `(scope, name)` already exists.
    pub async fn create(
        &self,
        scope: &str,
        name: &str,
        description: &str,
        parent_role_id: Option<Uuid>,
    ) -> Result<Uuid> {
        let res = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO roles (scope, name, description, parent_role_id) \
             VALUES ($1, $2, $3, $4) RETURNING id",
        )
        .bind(scope)
        .bind(name)
        .bind(description)
        .bind(parent_role_id)
        .fetch_one(&self.db.pool)
        .await;
        match res {
            Ok(id) => {
                self.bus.publish(RolesChanged);
                Ok(id)
            }
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Err(
                egret_core::Error::Conflict(format!("role already exists: {scope}/{name}")),
            ),
            Err(e) => Err(db_err(e)),
        }
    }

    /// Set (or clear) a role's parent. Validates same-scope and acyclic in the
    /// app layer (the DB trigger re-checks). Publishes `RolesChanged` so the PDP
    /// recomputes permission closures.
    pub async fn set_parent(&self, id: Uuid, parent: Option<Uuid>) -> Result<()> {
        // Serialize ALL role-graph parent mutations on a transaction-scoped advisory
        // lock so two concurrent `set_parent` calls can't form a cycle via write-skew
        // under READ COMMITTED (the cycle-check + the UPDATE become atomic w.r.t. any
        // other set_parent). Only this method takes the lock, so contention is nil.
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(ROLE_GRAPH_LOCK_KEY)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        if let Some(pid) = parent {
            let child = self
                .get(id)
                .await?
                .ok_or_else(|| egret_core::Error::NotFound(format!("role {id}")))?;
            let parent_row = self.get(pid).await?.ok_or_else(|| {
                egret_core::Error::InvalidInput(format!("parent role {pid} not found"))
            })?;
            if parent_row.scope != child.scope {
                return Err(egret_core::Error::InvalidInput(
                    "role parent must share scope".into(),
                ));
            }
            // Walk up from parent; a hit on `id` is a cycle.
            let mut cur = Some(pid);
            let mut hops = 0;
            while let Some(c) = cur {
                if c == id {
                    return Err(egret_core::Error::InvalidInput("role parent cycle".into()));
                }
                hops += 1;
                if hops > 64 {
                    return Err(egret_core::Error::InvalidInput(
                        "role parent chain too deep".into(),
                    ));
                }
                cur = self.get(c).await?.and_then(|r| r.parent_role_id);
            }
        }
        sqlx::query("UPDATE roles SET parent_role_id = $1 WHERE id = $2")
            .bind(parent)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        self.bus.publish(RolesChanged);
        Ok(())
    }

    /// Update a role's description (name/scope are immutable after creation).
    pub async fn update_description(&self, id: Uuid, description: &str) -> Result<()> {
        sqlx::query("UPDATE roles SET description = $1 WHERE id = $2")
            .bind(description)
            .bind(id)
            .execute(&self.db.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    /// Delete a role (cascades to `role_permissions` and `access_grants`).
    pub async fn delete(&self, id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM roles WHERE id = $1")
            .bind(id)
            .execute(&self.db.pool)
            .await
            .map_err(db_err)?;
        self.bus.publish(RolesChanged);
        Ok(())
    }

    /// Replace a role's permission set with `perms`, in one transaction.
    pub async fn set_permissions(&self, id: Uuid, perms: &[String]) -> Result<()> {
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        sqlx::query("DELETE FROM role_permissions WHERE role_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        for perm in perms {
            sqlx::query(
                "INSERT INTO role_permissions (role_id, permission) VALUES ($1, $2) \
                 ON CONFLICT DO NOTHING",
            )
            .bind(id)
            .bind(perm)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }
        tx.commit().await.map_err(db_err)?;
        self.bus.publish(RolesChanged);
        Ok(())
    }

    /// Assign a role to a principal (idempotent). Writes the unified access_grants
    /// store directly (platform scope). Idempotency is guarded with NOT EXISTS
    /// rather than ON CONFLICT: the UNIQUE index includes `scope_id`, which is NULL
    /// for platform grants, and Postgres treats NULLs as distinct — so ON CONFLICT
    /// would never fire and re-assign would duplicate the row.
    pub async fn assign(&self, ptype: &str, pid: Uuid, role_id: Uuid) -> Result<()> {
        sqlx::query(
            "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
             SELECT $1, $2, $3, 'platform', NULL \
             WHERE NOT EXISTS ( \
               SELECT 1 FROM access_grants \
               WHERE principal_type = $1 AND principal_id = $2 AND role_id = $3 AND scope_type = 'platform')",
        )
        .bind(ptype)
        .bind(pid)
        .bind(role_id)
        .execute(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    /// Remove a platform role assignment from a principal.
    pub async fn unassign(&self, ptype: &str, pid: Uuid, role_id: Uuid) -> Result<()> {
        sqlx::query(
            "DELETE FROM access_grants \
             WHERE principal_type = $1 AND principal_id = $2 AND role_id = $3 AND scope_type = 'platform'",
        )
        .bind(ptype)
        .bind(pid)
        .bind(role_id)
        .execute(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }
}

fn db_err(e: sqlx::Error) -> egret_core::Error {
    egret_core::Error::Internal(anyhow::anyhow!(e))
}
