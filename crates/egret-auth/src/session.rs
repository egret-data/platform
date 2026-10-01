//! Server-side sessions with two-tier expiry (absolute cap + sliding idle window).

use chrono::{DateTime, Duration, Utc};
use egret_core::{Principal, Result, UserId, WorkspaceId};
use egret_metastore::Db;
use uuid::Uuid;

use crate::token::{generate_token, hash_token};

#[derive(Clone)]
pub struct SessionService {
    db: Db,
    absolute_ttl: Duration,
    idle_ttl: Duration,
    impersonations: crate::impersonation::ImpersonationRepo,
}

/// What `create` returns: the raw token (for the cookie) plus expiry timestamps.
pub struct IssuedSession {
    pub token: String,
    pub absolute_expires_at: DateTime<Utc>,
    pub idle_expires_at: DateTime<Utc>,
}

impl SessionService {
    pub fn new(db: Db, absolute_ttl: Duration, idle_ttl: Duration) -> Self {
        let impersonations = crate::impersonation::ImpersonationRepo::new(db.clone());
        Self {
            db,
            absolute_ttl,
            idle_ttl,
            impersonations,
        }
    }

    /// Create a fresh session for a user. Caller sets the cookie from the returned token.
    pub async fn create(
        &self,
        user_id: UserId,
        user_agent: &str,
        ip: &str,
    ) -> Result<IssuedSession> {
        let now = Utc::now();
        let absolute = now + self.absolute_ttl;
        let idle = now + self.idle_ttl;
        let token = generate_token();
        let token_hash = hash_token(&token);

        sqlx::query(
            "INSERT INTO sessions \
             (token_hash, user_id, absolute_expires_at, idle_expires_at, user_agent, ip) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&token_hash)
        .bind(user_id.0)
        .bind(absolute)
        .bind(idle)
        .bind(user_agent)
        .bind(ip)
        .execute(&self.db.pool)
        .await
        .map_err(db_err)?;

        Ok(IssuedSession {
            token,
            absolute_expires_at: absolute,
            idle_expires_at: idle,
        })
    }

    /// Validate a raw token. Returns the `Principal` and slides the idle window, or
    /// `Ok(None)` if missing/expired. Platform roles are loaded for the principal.
    pub async fn validate(&self, token: &str) -> Result<Option<Principal>> {
        let token_hash = hash_token(token);
        let now = Utc::now();

        let row = sqlx::query_as::<_, ValidateRow>(
            "SELECT s.id AS session_id, s.user_id, u.email, u.status \
             FROM sessions s JOIN users u ON u.id = s.user_id \
             WHERE s.token_hash = $1 AND s.absolute_expires_at > $2 AND s.idle_expires_at > $2",
        )
        .bind(&token_hash)
        .bind(now)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;

        let Some(row) = row else { return Ok(None) };
        if row.status != "active" {
            return Ok(None);
        }

        // Slide the idle window.
        let new_idle = now + self.idle_ttl;
        sqlx::query(
            "UPDATE sessions SET idle_expires_at = $1, last_seen_at = $2 WHERE token_hash = $3",
        )
        .bind(new_idle)
        .bind(now)
        .bind(&token_hash)
        .execute(&self.db.pool)
        .await
        .map_err(db_err)?;

        // Impersonation overlay: if this admin session is currently impersonating,
        // resolve to the TARGET's principal and carry the admin as `impersonator`.
        // Only ever consult the overlay for THIS admin session's own row
        // (`row.session_id`) — never the target's. Expired overlays are ignored by
        // `active_for_session` (auto-return to admin), so this branch only runs for a
        // live overlay. If the target user is not active (disabled since the overlay
        // was started), the overlay is treated as inert and we fall through to the
        // admin principal — impersonating a disabled user must not resolve.
        if let Some(ov) = self
            .impersonations
            .active_for_session(row.session_id)
            .await?
        {
            let target = UserId(ov.target_user_id);
            let target_row: Option<(String, String)> =
                sqlx::query_as("SELECT email, status FROM users WHERE id = $1")
                    .bind(ov.target_user_id)
                    .fetch_optional(&self.db.pool)
                    .await
                    .map_err(db_err)?;
            if let Some((target_email, target_status)) = target_row {
                if target_status == "active" {
                    // `impersonator_id` == the session owner (`row.user_id`), by
                    // construction — the overlay is always attached to the admin's own
                    // session — so `row.email` is the impersonator's email and no extra
                    // query is needed.
                    return Ok(Some(Principal {
                        user_id: target,
                        email: target_email,
                        platform_roles: self.platform_roles(target).await?,
                        permissions: self.permissions(target).await?,
                        group_ids: self.group_ids(target).await?,
                        impersonator: Some(egret_core::context::Impersonator {
                            user_id: UserId(ov.impersonator_id),
                            email: row.email.clone(),
                            workspace_scope: ov.workspace_scope_id.map(egret_core::WorkspaceId),
                            expires_at: ov.expires_at,
                        }),
                    }));
                }
            }
        }

        let platform_roles = self.platform_roles(UserId(row.user_id)).await?;
        let permissions = self.permissions(UserId(row.user_id)).await?;
        let group_ids = self.group_ids(UserId(row.user_id)).await?;
        Ok(Some(Principal {
            user_id: UserId(row.user_id),
            email: row.email,
            platform_roles,
            permissions,
            group_ids,
            impersonator: None,
        }))
    }

    /// Resolve the `sessions.id` for a raw session token (via its stored hash),
    /// or `None` if no such session row exists. Used by the impersonation
    /// endpoints to key the overlay on the admin's own session.
    pub async fn session_id_for_token(&self, token: &str) -> Result<Option<Uuid>> {
        let token_hash = hash_token(token);
        let id = sqlx::query_scalar::<_, Uuid>("SELECT id FROM sessions WHERE token_hash = $1")
            .bind(&token_hash)
            .fetch_optional(&self.db.pool)
            .await
            .map_err(db_err)?;
        Ok(id)
    }

    pub async fn revoke(&self, token: &str) -> Result<()> {
        let token_hash = hash_token(token);
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(&token_hash)
            .execute(&self.db.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    pub async fn revoke_all_for_user(&self, user_id: UserId) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE user_id = $1")
            .bind(user_id.0)
            .execute(&self.db.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    /// Public wrapper over `platform_roles` so endpoints can check a target
    /// user's platform roles.
    ///
    /// `validate_impersonation_target` reads it for EMPTINESS, not for a name: an
    /// impersonation target may hold no platform-scoped role grant at all, because
    /// such a grant is the only thing `egret_auth::grants::grants_for` admits when
    /// the PDP decides a `Resource::Platform`, and no `/api/control-plane` route
    /// builds a `WsContext` to confine a scoped overlay. See that function's ⚠
    /// paragraph for the measured chain.
    pub async fn platform_roles_of(&self, user_id: UserId) -> Result<Vec<String>> {
        self.platform_roles(user_id).await
    }

    /// True if `user_id` holds a `ws_admin` grant on `workspace_id` (direct or via a
    /// group), non-expired. A direct `access_grants` query — egret-auth can't import
    /// egret-workspace's `Authorizer` (that would be a dependency cycle), and this
    /// authority IS just a grant row. Modeled on `platform_roles` (same
    /// group-membership union and `expires_at` filter), narrowed to
    /// `scope_type = 'workspace' AND scope_id = $ws AND r.name = 'ws_admin'`.
    ///
    /// Used by the OIDC step-up callback to re-check a workspace-scoped
    /// impersonation's authority at completion time (closes the ws-path demotion
    /// TOCTOU, Task 9).
    pub async fn ws_admin_of(&self, user_id: UserId, workspace_id: WorkspaceId) -> Result<bool> {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS ( \
               SELECT 1 FROM access_grants ag \
               JOIN roles r ON r.id = ag.role_id \
               WHERE ag.scope_type = 'workspace' AND ag.scope_id = $2 AND r.name = 'ws_admin' \
                 AND (ag.expires_at IS NULL OR ag.expires_at > now()) \
                 AND ( (ag.principal_type = 'user'  AND ag.principal_id = $1) \
                    OR (ag.principal_type = 'group' AND ag.principal_id IN \
                           (SELECT group_id FROM user_groups WHERE user_id = $1)) ) )",
        )
        .bind(user_id.0)
        .bind(workspace_id.0)
        .fetch_one(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(exists)
    }

    /// Platform roles for the user (direct + via group membership).
    async fn platform_roles(&self, user_id: UserId) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT DISTINCT r.name FROM access_grants ag \
             JOIN roles r ON r.id = ag.role_id \
             WHERE ag.scope_type = 'platform' \
               AND (ag.expires_at IS NULL OR ag.expires_at > now()) \
               AND ( (ag.principal_type = 'user'  AND ag.principal_id = $1) \
                  OR (ag.principal_type = 'group' AND ag.principal_id IN \
                         (SELECT group_id FROM user_groups WHERE user_id = $1)) )",
        )
        .bind(user_id.0)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(|(n,)| n).collect())
    }

    /// Effective permission keys for the user: role_permissions over every platform
    /// role assigned directly or via a group, **expanded up the role-hierarchy parent
    /// chain** so an inherited permission (from a parent role) is surfaced to the
    /// frontend's permission-aware UI gating — matching what the PDP enforces via its
    /// closure. (FE gating only; enforcement is always the PDP.)
    async fn permissions(&self, user_id: UserId) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "WITH RECURSIVE granted AS ( \
               SELECT ag.role_id FROM access_grants ag \
               WHERE ag.scope_type = 'platform' \
                 AND (ag.expires_at IS NULL OR ag.expires_at > now()) \
                 AND ( (ag.principal_type = 'user'  AND ag.principal_id = $1) \
                    OR (ag.principal_type = 'group' AND ag.principal_id IN \
                           (SELECT group_id FROM user_groups WHERE user_id = $1)) ) \
             ), \
             closure AS ( \
               SELECT role_id FROM granted \
               UNION \
               SELECT r.parent_role_id FROM roles r \
                 JOIN closure c ON r.id = c.role_id \
                WHERE r.parent_role_id IS NOT NULL \
             ) \
             SELECT DISTINCT rp.permission \
               FROM closure c JOIN role_permissions rp ON rp.role_id = c.role_id",
        )
        .bind(user_id.0)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(|(p,)| p).collect())
    }

    /// Group ids the user is a direct member of.
    async fn group_ids(&self, user_id: UserId) -> Result<Vec<egret_core::ids::GroupId>> {
        let rows: Vec<(Uuid,)> =
            sqlx::query_as("SELECT group_id FROM user_groups WHERE user_id = $1")
                .bind(user_id.0)
                .fetch_all(&self.db.pool)
                .await
                .map_err(db_err)?;
        Ok(rows
            .into_iter()
            .map(|(g,)| egret_core::ids::GroupId(g))
            .collect())
    }
}

#[derive(sqlx::FromRow)]
struct ValidateRow {
    session_id: Uuid,
    user_id: Uuid,
    email: String,
    status: String,
}

fn db_err(e: sqlx::Error) -> egret_core::Error {
    egret_core::Error::Internal(anyhow::anyhow!(e))
}
