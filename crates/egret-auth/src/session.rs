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
        //
        // The same applies to a target who has GAINED platform authority since the
        // overlay began. The principal below is rebuilt from live tables on every
        // request, so `validate_impersonation_target`'s creation-time check was
        // time-dependent and therefore escapable (whole-branch review, H-1): an IdP
        // group change replayed by `sync_user_oidc_groups`, or a console grant,
        // needs no permission from the admin and handed a `ws_admin`'s own token a
        // principal carrying platform permissions that no `/api/control-plane` route
        // confines. So the SAME predicate the creation guard uses —
        // `overlay_refused_by_platform_authority`, called, never restated — is
        // re-evaluated here, and when it refuses, the overlay is inert and we fall
        // through to the admin principal exactly as for a disabled target.
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
                    // Hoisted out of the `Principal` literal below, which already
                    // fetched it on that line — so re-checking the platform-authority
                    // rule costs no extra query.
                    let target_platform_roles = self.platform_roles(target).await?;
                    // Refused ⇒ no `return` happens here and control reaches the
                    // admin-principal tail below: the overlay is inert, nothing is
                    // written, and the admin is simply an admin again.
                    if !crate::impersonation::overlay_refused_by_platform_authority(
                        ov.workspace_scope_id.is_some(),
                        &target_platform_roles,
                    ) {
                        // `impersonator_id` == the session owner (`row.user_id`), by
                        // construction — the overlay is always attached to the admin's own
                        // session — so `row.email` is the impersonator's email and no extra
                        // query is needed.
                        return Ok(Some(Principal {
                            user_id: target,
                            email: target_email,
                            platform_roles: target_platform_roles,
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

    /// The user's role names in ONE workspace (direct + via group, non-expired). An
    /// empty vector means the user is not a member of it.
    ///
    /// A direct `access_grants` query for the same reason [`Self::ws_admin_of`] is
    /// one — egret-auth cannot import egret-workspace's `Authorizer` without a
    /// dependency cycle, and this authority IS just a grant row. Same shape as
    /// `egret_workspace::rbac::Authorizer::workspace_roles_for`.
    ///
    /// Read by `impersonation::validate_ws_impersonation_target_membership`, which is
    /// the single place the "member of A and not a `ws_admin` of A" rule lives, so
    /// the OIDC step-up callback applies the same target rule as the inline
    /// `ws_impersonate_start` (whole-branch review, M-1).
    pub async fn workspace_roles_of(
        &self,
        user_id: UserId,
        workspace_id: WorkspaceId,
    ) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT DISTINCT r.name FROM access_grants ag \
             JOIN roles r ON r.id = ag.role_id \
             WHERE ag.scope_type = 'workspace' AND ag.scope_id = $2 \
               AND (ag.expires_at IS NULL OR ag.expires_at > now()) \
               AND ( (ag.principal_type = 'user'  AND ag.principal_id = $1) \
                  OR (ag.principal_type = 'group' AND ag.principal_id IN \
                         (SELECT group_id FROM user_groups WHERE user_id = $1)) )",
        )
        .bind(user_id.0)
        .bind(workspace_id.0)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(|(n,)| n).collect())
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
