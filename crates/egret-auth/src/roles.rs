//! Postgres-backed store for platform RBAC roles, their permissions, and
//! principal (user/group) assignments.

use crate::events::RolesChanged;
use egret_core::ids::UserId;
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

impl RoleRepo {
    /// The pool behind this repo, so a caller can own a TRANSACTION spanning a
    /// role-membership write and its consequences.
    ///
    /// **SP4c Task 3.** A platform role grant changes who holds a CLEARING role,
    /// and that must be accompanied by a policy recompile in the SAME
    /// transaction — a stored-but-uncompiled membership change is what leaves an
    /// engine enforcing the previous holder set. The recompile lives in
    /// `egret-perm-compile`, which depends on this crate, so it cannot be called
    /// from here without a dependency cycle; the caller therefore drives both
    /// halves, through [`Self::assign_in_tx`]/[`Self::unassign_in_tx`] and
    /// `egret_perm_compile::role_membership`.
    pub fn pool(&self) -> &sqlx::PgPool {
        &self.db.pool
    }
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

    /// User ids that hold the `platform_admin` role at platform scope — both
    /// DIRECT user grants AND users who hold it via a GROUP grant (group-granted
    /// admins ARE expanded through `user_groups`; see the query below). Used to
    /// count/enumerate the effective platform admins (e.g. the last-admin guard).
    pub async fn platform_admin_user_ids(&self) -> Result<Vec<UserId>> {
        // Both DIRECT user grants AND users who hold platform_admin via a GROUP
        // (OIDC groups sync into `user_groups`). The group arm expands each
        // group-scoped grant to its members, mirroring the group-membership
        // expansion in `SessionService` (see session.rs). This is what lets a
        // group-granted platform admin (e.g. an OIDC `platform-admins` group) be
        // auto-seeded a Ranger policy on engine provision.
        let rows = sqlx::query_scalar::<_, Uuid>(
            "SELECT DISTINCT pra.principal_id FROM access_grants pra \
               JOIN roles r ON r.id = pra.role_id \
              WHERE r.name = 'platform_admin' \
                AND pra.scope_type = 'platform' \
                AND pra.principal_type = 'user' \
             UNION \
             SELECT DISTINCT ug.user_id FROM access_grants pra \
               JOIN roles r ON r.id = pra.role_id \
               JOIN user_groups ug ON ug.group_id = pra.principal_id \
              WHERE r.name = 'platform_admin' \
                AND pra.scope_type = 'platform' \
                AND pra.principal_type = 'group'",
        )
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(UserId).collect())
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
    ///
    /// **Not the path the control plane takes, and deliberately so.** Deleting a
    /// role is a role-MEMBERSHIP write wearing a different name: migration
    /// `0068_access_grants.sql` makes `access_grants.role_id` `ON DELETE
    /// CASCADE`, so this one statement revokes the role from every holder at
    /// once. If the role was clearing a classification, the compiled
    /// `denyExceptions` goes on naming the ex-holders until something
    /// recompiles, and they go on reading the classified column — the
    /// union-not-intersection failure SP4c exists to close, re-opened by a
    /// revocation that looked like a delete. Use
    /// `egret_perm_compile::delete_platform_role_and_recompile`, which takes
    /// this crate's [`Self::delete_in_tx`] inside the same transaction as the
    /// recompile. This method remains for callers with no engine consequence
    /// (tests, fixtures).
    pub async fn delete(&self, id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM roles WHERE id = $1")
            .bind(id)
            .execute(&self.db.pool)
            .await
            .map_err(db_err)?;
        self.bus.publish(RolesChanged);
        Ok(())
    }

    /// [`Self::delete`] on a caller-supplied connection — see
    /// [`Self::assign_in_tx`] for why the recompile cannot live in this crate,
    /// and [`Self::delete`] for why a delete needs one at all.
    pub async fn delete_in_tx(conn: &mut sqlx::PgConnection, id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM roles WHERE id = $1")
            .bind(id)
            .execute(&mut *conn)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    /// Announce that the role graph changed, for a caller that owned the
    /// transaction itself and so could not let a `RoleRepo` method publish.
    /// Called AFTER the commit: the PDP invalidates its cache on this event, and
    /// publishing before the commit would let it re-read the pre-commit state.
    pub fn publish_roles_changed(&self) {
        self.bus.publish(RolesChanged);
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

    /// [`Self::assign`] on a caller-supplied connection — the `*_in_tx` half of
    /// the pair, so a caller can commit the grant and its consequences together.
    ///
    /// **SP4c Task 3 needs this and could not put the consequence here.** A
    /// platform role grant changes who holds a CLEARING role, and egret
    /// enumerates a role's holders at compile time (a role NAME matches nobody
    /// in any engine egret provisions — D0/D0b), so the grant must be
    /// accompanied by a recompile in the same transaction. That recompile lives
    /// in `egret-perm-compile`, which depends on THIS crate — so it cannot be
    /// called from here without a dependency cycle. The caller owns the
    /// transaction instead; see `egret_controlplane::access`'s role handlers for
    /// the shape, and `egret_perm_compile::affected_services_for_role_membership`
    /// for the one trigger both consumers share.
    pub async fn assign_in_tx(
        conn: &mut sqlx::PgConnection,
        ptype: &str,
        pid: Uuid,
        role_id: Uuid,
    ) -> Result<()> {
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
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    /// [`Self::unassign`] on a caller-supplied connection — see
    /// [`Self::assign_in_tx`] for why the recompile cannot live in this crate.
    pub async fn unassign_in_tx(
        conn: &mut sqlx::PgConnection,
        ptype: &str,
        pid: Uuid,
        role_id: Uuid,
    ) -> Result<()> {
        sqlx::query(
            "DELETE FROM access_grants \
             WHERE principal_type = $1 AND principal_id = $2 AND role_id = $3 AND scope_type = 'platform'",
        )
        .bind(ptype)
        .bind(pid)
        .bind(role_id)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    /// The `roles.name` of `role_id`, or `None`. The name is what
    /// `perm.grant_principals` and `classification_policies.required_roles` both
    /// store, so it is the key every clearance/role-grant trigger is looked up
    /// by — never the id.
    pub async fn name_of_in_tx(
        conn: &mut sqlx::PgConnection,
        role_id: Uuid,
    ) -> Result<Option<String>> {
        sqlx::query_scalar("SELECT name FROM roles WHERE id = $1")
            .bind(role_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(db_err)
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

pub(crate) fn db_err(e: sqlx::Error) -> egret_core::Error {
    egret_core::Error::Internal(anyhow::anyhow!(e))
}

/// The engine admins: every ACTIVE user holding `platform_admin` at platform
/// scope, directly or through a group (the two arms of
/// [`RoleRepo::platform_admin_user_ids`]), ascending by id.
///
/// The one definition of "engine admin" (username-takeover spec 2026-10-23
/// §4.2): engine apply seeds the `egret-admin - …` policies from it and every
/// recompile converges them to it. A disabled admin is left out: the policy
/// means "may do anything on this engine", and a disabled account may do
/// nothing in egret; leaving them in would keep an already-minted engine JWT
/// (or an alert running as them) at ALL access.
pub async fn engine_admin_user_ids_via(conn: &mut sqlx::PgConnection) -> Result<Vec<UserId>> {
    let rows = sqlx::query_scalar::<_, Uuid>(
        "SELECT u.id FROM users u \
          WHERE u.status = 'active' \
            AND (EXISTS (SELECT 1 FROM access_grants g JOIN roles r ON r.id = g.role_id \
                          WHERE r.name = 'platform_admin' AND g.scope_type = 'platform' \
                            AND g.principal_type = 'user' AND g.principal_id = u.id) \
              OR EXISTS (SELECT 1 FROM access_grants g JOIN roles r ON r.id = g.role_id \
                           JOIN user_groups ug ON ug.group_id = g.principal_id \
                          WHERE r.name = 'platform_admin' AND g.scope_type = 'platform' \
                            AND g.principal_type = 'group' AND ug.user_id = u.id)) \
          ORDER BY u.id",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(rows.into_iter().map(UserId).collect())
}
