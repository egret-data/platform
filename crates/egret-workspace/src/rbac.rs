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
            "SELECT DISTINCT r.name FROM access_grants ag \
             JOIN roles r ON r.id = ag.role_id \
             WHERE ag.scope_type = 'workspace' AND ag.scope_id = $1 \
               AND (ag.expires_at IS NULL OR ag.expires_at > now()) \
               AND ( (ag.principal_type = 'user'  AND ag.principal_id = $2) \
                  OR (ag.principal_type = 'group' AND ag.principal_id IN \
                         (SELECT group_id FROM user_groups WHERE user_id = $2)) )",
        )
        .bind(ws.0)
        .bind(user_id.0)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(|(r,)| r).collect())
    }

    /// True if `user_id` (directly or via a group) holds an object-scoped grant on
    /// any object that lives in workspace `ws`. Used by `WsContext` to admit an
    /// "object-guest" — a non-member share recipient — so per-object handlers can
    /// run `require_object`. This carries NO workspace permissions; workspace-level
    /// `require` still denies them.
    pub async fn has_object_grant_in_workspace(
        &self,
        user_id: UserId,
        ws: WorkspaceId,
    ) -> Result<bool> {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS( \
               SELECT 1 FROM access_grants ag \
               WHERE ( (ag.principal_type = 'user'  AND ag.principal_id = $1) \
                    OR (ag.principal_type = 'group' AND ag.principal_id IN \
                          (SELECT group_id FROM user_groups WHERE user_id = $1)) ) \
                 AND ag.scope_type = 'object' \
                 AND ag.effect = 'allow' \
                 AND (ag.expires_at IS NULL OR ag.expires_at > now()) \
                 AND ( (ag.object_type = 'folder'    AND ag.scope_id IN (SELECT id FROM analytics.folders    WHERE workspace_id = $2)) \
                    OR (ag.object_type = 'dashboard' AND ag.scope_id IN (SELECT id FROM analytics.dashboards WHERE workspace_id = $2)) \
                    OR (ag.object_type = 'chart'     AND ag.scope_id IN (SELECT id FROM analytics.charts     WHERE workspace_id = $2)) \
                    OR (ag.object_type = 'dataset'   AND ag.scope_id IN (SELECT id FROM analytics.datasets   WHERE workspace_id = $2)) \
                    OR (ag.object_type = 'notebook'  AND ag.scope_id IN (SELECT id FROM analytics.notebooks  WHERE workspace_id = $2)) \
                    OR (ag.object_type = 'saved_query' AND ag.scope_id IN (SELECT id FROM analytics.saved_queries WHERE workspace_id = $2)) \
                    OR (ag.object_type = 'app'       AND ag.scope_id IN (SELECT id FROM analytics.apps       WHERE workspace_id = $2)) ) )",
        )
        .bind(user_id.0)
        .bind(ws.0)
        .fetch_one(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(exists)
    }

    /// Replace the user's platform role assignments with the supplied (validated) role names.
    pub async fn replace_platform_roles(&self, user_id: UserId, roles: &[String]) -> Result<()> {
        for r in roles {
            if !PLATFORM_ROLES.contains(&r.as_str()) {
                return Err(Error::InvalidInput(format!("unknown platform role: {r}")));
            }
        }
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;

        // **SP4c Task 3, input 3: who holds the clearing role.** This method
        // REPLACES a user's whole platform role set, so the affected services are
        // the union over the roles they are about to LOSE and the ones they are
        // about to gain — the old half read before the DELETE, because after it
        // there is nothing left to read it from. A role whose holders are
        // enumerated into a clearance's `denyExceptions` or a `role` grant's users
        // goes stale here exactly as it does at the single-role grant handler;
        // this path just changes several at once.
        let mut affected: Vec<i64> = Vec::new();
        let losing: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT r.name FROM access_grants ag JOIN roles r ON r.id = ag.role_id \
              WHERE ag.principal_type = 'user' AND ag.principal_id = $1 \
                AND ag.scope_type = 'platform' ORDER BY r.name",
        )
        .bind(user_id.0)
        .fetch_all(&mut *tx)
        .await
        .map_err(db_err)?;
        for name in losing
            .iter()
            .map(String::as_str)
            .chain(roles.iter().map(String::as_str))
        {
            affected.extend(
                egret_perm_compile::affected_services_for_role_membership(&mut tx, name)
                    .await
                    .map_err(|e| Error::Internal(anyhow::anyhow!(e)))?,
            );
        }
        affected.sort_unstable();
        affected.dedup();
        // Ascending, and BEFORE any `access_grants` row is touched — the global
        // lock order `egret_perm_compile::lock_services_in_tx` documents, which is
        // what turns two overlapping recompiles into a queue instead of a deadlock.
        debug_assert!(
            affected.is_sorted(),
            "profile locks must be taken ascending"
        );
        egret_perm_compile::lock_services_in_tx(&mut tx, &affected)
            .await
            .map_err(|e| Error::Internal(anyhow::anyhow!(e)))?;

        sqlx::query(
            "DELETE FROM access_grants \
             WHERE principal_type = 'user' AND principal_id = $1 AND scope_type = 'platform'",
        )
        .bind(user_id.0)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        for r in roles {
            // NOT EXISTS rather than ON CONFLICT: platform grants have a NULL
            // scope_id, which the UNIQUE index treats as distinct, so ON CONFLICT
            // would never dedupe a repeated role name in `roles`.
            sqlx::query(
                "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
                 SELECT 'user', $1, ro.id, 'platform', NULL FROM roles ro \
                 WHERE ro.scope = 'platform' AND ro.name = $2 \
                   AND NOT EXISTS ( \
                     SELECT 1 FROM access_grants ag \
                     WHERE ag.principal_type = 'user' AND ag.principal_id = $1 \
                       AND ag.role_id = ro.id AND ag.scope_type = 'platform')",
            )
            .bind(user_id.0)
            .bind(r)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }
        // A recompile FAILURE fails the whole replace, deliberately: a committed
        // role set whose engine policies were never rewritten is worse than a
        // rejected one, because egret's UI shows the new set as in force.
        egret_perm_compile::recompile_services_in_tx(&mut tx, &affected)
            .await
            .map_err(|e| Error::Internal(anyhow::anyhow!(e)))?;
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }
}
