//! Postgres-backed workspace membership.
//!
//! **Every membership write recompiles the permission model** (SP4a D3). A
//! catalog assigned to a workspace carries an implicit `read` grant whose
//! principal is that workspace's members, and D7 measured that a Ranger GROUP
//! item matches nobody on any engine egret provisions — so that grant compiles
//! to the member list **enumerated at compile time**
//! (`egret_perm_compile::members`). The enumeration is stale the instant
//! someone joins or leaves, and the two directions are not symmetric: a missed
//! JOIN denies someone who should have access, a missed LEAVE keeps granting
//! someone who should not. So `add_with_expiry` and `remove` — the two
//! functions every membership path in egret funnels through — recompile the
//! affected profiles in the SAME transaction as the `access_grants` write,
//! the shape `egret_perm::sync_assignment_grants_in_tx` and
//! `egret-bi::catalog_tags` already established.
//!
//! The profile locks are taken FIRST, ascending, before any row this function
//! writes — `egret_perm_compile::lock_services_in_tx`'s own invariant, checked
//! by a `debug_assert!` at that gate.

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
        self.add_with_expiry(ws, ptype, pid, role, by, None).await
    }

    /// Like [`add`](Self::add), but stamps a time-boxed `expires_at` on the grant
    /// (P2a). `None` = never expires (identical to `add`). Used by the
    /// access-request approval path to grant time-limited membership.
    pub async fn add_with_expiry(
        &self,
        ws: WorkspaceId,
        ptype: &str,
        pid: Uuid,
        role: &str,
        by: UserId,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
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
        // Which Ranger profiles enumerate this workspace's membership, and
        // their locks, BEFORE the write — the affected set is derived from the
        // `perm` model, which this write does not change, so reading it first
        // is correct and keeps every lock this request needs acquired in one
        // ascending pass (see the module doc).
        // SP4c Task 3: the roles this write touches — the one being granted, plus
        // any this principal already holds here and is about to lose.
        let mut touched: Vec<String> = current_roles(&mut tx, ws, ptype, pid).await?;
        touched.push(role.to_string());
        let services = Self::affected_for_membership_write(&mut tx, ws, &touched).await?;
        if !services.is_empty() {
            egret_perm_compile::lock_services_in_tx(&mut tx, &services)
                .await
                .map_err(compile_err)?;
        }
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
               (principal_type, principal_id, role_id, scope_type, scope_id, granted_by, expires_at) \
             SELECT $1, $2, r.id, 'workspace', $3, $4, $6 FROM roles r \
             WHERE r.scope = 'workspace' AND r.name = $5 \
             ON CONFLICT DO NOTHING",
        )
        .bind(ptype)
        .bind(pid)
        .bind(ws.0)
        .bind(by.0)
        .bind(role)
        .bind(expires_at)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        if !services.is_empty() {
            egret_perm_compile::recompile_services_in_tx(&mut tx, &services)
                .await
                .map_err(compile_err)?;
        }
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }

    /// Remove a principal's workspace grant — and recompile, in the same
    /// transaction, every Ranger profile whose compiled policies enumerate
    /// this workspace's membership.
    ///
    /// **This is the direction that fails OPEN when it is missed.** A removed
    /// member whose username is still named in a live access policy keeps
    /// reading the workspace's catalogs through the engine, with egret's own
    /// member list showing them gone. That is why the recompile shares the
    /// transaction rather than following it.
    pub async fn remove(&self, ws: WorkspaceId, ptype: &str, pid: Uuid) -> Result<()> {
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        // SP4c Task 3: read the roles being removed BEFORE the DELETE — after it
        // there is nothing left to read them from.
        let touched = current_roles(&mut tx, ws, ptype, pid).await?;
        let services = Self::affected_for_membership_write(&mut tx, ws, &touched).await?;
        if !services.is_empty() {
            egret_perm_compile::lock_services_in_tx(&mut tx, &services)
                .await
                .map_err(compile_err)?;
        }
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
        if !services.is_empty() {
            egret_perm_compile::recompile_services_in_tx(&mut tx, &services)
                .await
                .map_err(compile_err)?;
        }
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }

    /// The workspace-membership affected set, WIDENED by SP4c Task 3 with the
    /// role-membership set for every workspace role this write touches.
    ///
    /// **Why a workspace role grant is a write to a CLEARING role's membership.**
    /// `roles` is `UNIQUE (scope, name)`, so a workspace role and a platform role
    /// may share a name — and `egret_perm_compile`'s `role_usernames`, which
    /// enumerates a role's holders for a clearance's `denyExceptions` and for a
    /// `role` grant's users, matches on `r.name` with **no scope filter**,
    /// deliberately (an engine read carries no workspace context on any engine, so
    /// narrowing to platform scope would DENY people Cedar clears). So granting
    /// someone the workspace role `analyst` makes them a holder of `analyst` as
    /// far as every compiled clearance is concerned, and the enumeration goes
    /// stale exactly as a platform grant's does.
    ///
    /// `roles` is the set of role NAMES this write adds or removes. Both halves
    /// are needed: the removed one because a stale REVOKE fails open, the added
    /// one because a stale GRANT denies someone who should be cleared.
    async fn affected_for_membership_write(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        ws: WorkspaceId,
        roles: &[String],
    ) -> Result<Vec<i64>> {
        let mut services = egret_perm_compile::affected_services_for_workspace_members(tx, ws.0)
            .await
            .map_err(compile_err)?;
        for role in roles {
            services.extend(
                egret_perm_compile::affected_services_for_role_membership(tx, role)
                    .await
                    .map_err(compile_err)?,
            );
        }
        // Ascending and deduplicated: `lock_services_in_tx` takes one row lock per
        // id in the order given, and that order is what turns two overlapping
        // recompiles into a queue instead of a deadlock.
        services.sort_unstable();
        services.dedup();
        debug_assert!(
            services.is_sorted(),
            "profile locks must be taken ascending"
        );
        Ok(services)
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

    /// Resolve a principal identifier to its id: a user by email, a group by name.
    /// `users.email` is citext so the match is case-insensitive.
    pub async fn resolve_principal(&self, ptype: &str, ident: &str) -> Result<Option<Uuid>> {
        let sql = match ptype {
            "user" => "SELECT id FROM users WHERE email = $1",
            "group" => "SELECT id FROM groups WHERE name = $1",
            _ => return Err(Error::InvalidInput(format!("bad principal_type: {ptype}"))),
        };
        sqlx::query_scalar(sql)
            .bind(ident)
            .fetch_optional(&self.db.pool)
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

/// A recompile failure is a downstream policy failure, not a bad request: the
/// membership the caller asked for is perfectly valid, and the `?` that
/// carries this rolls the membership write back with it rather than leaving
/// egret and the engine disagreeing. Rendered with the underlying message so
/// an operator sees WHICH profile refused (a ceiling, a missing tag service)
/// rather than an opaque 500.
fn compile_err(e: impl std::fmt::Display) -> Error {
    Error::Internal(anyhow::anyhow!("permission recompile failed: {e}"))
}

/// The workspace-role NAMES `(ptype, pid)` currently holds in `ws`, read on the
/// caller's transaction — SP4c Task 3 needs them before a write that replaces or
/// removes them, because afterwards there is nothing left to read.
async fn current_roles(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ws: WorkspaceId,
    ptype: &str,
    pid: Uuid,
) -> Result<Vec<String>> {
    sqlx::query_scalar(
        "SELECT DISTINCT r.name FROM access_grants ag JOIN roles r ON r.id = ag.role_id \
          WHERE ag.principal_type = $1 AND ag.principal_id = $2 \
            AND ag.scope_type = 'workspace' AND ag.scope_id = $3 ORDER BY r.name",
    )
    .bind(ptype)
    .bind(pid)
    .bind(ws.0)
    .fetch_all(&mut **tx)
    .await
    .map_err(db_err)
}
