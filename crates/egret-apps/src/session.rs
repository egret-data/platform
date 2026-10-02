//! Per-marimo-session bearer tokens for the query gateway (spec §4.1).

use std::time::Duration;

use egret_core::{token, UserId, WorkspaceId};
use egret_metastore::Db;
use sqlx::Row;
use uuid::Uuid;

pub const TOKEN_PREFIX: &str = "ega_";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    Edit,
    Run,
}

impl AppMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Edit => "edit",
            Self::Run => "run",
        }
    }

    fn parse(s: &str) -> egret_core::Result<Self> {
        match s {
            "edit" => Ok(Self::Edit),
            "run" => Ok(Self::Run),
            other => Err(anyhow::anyhow!("unknown app session mode {other}").into()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct NewAppSession {
    pub user_id: UserId,
    pub workspace_id: WorkspaceId,
    pub app_id: Uuid,
    /// `None` exactly when `mode` is `Edit` (enforced by a CHECK).
    pub version_id: Option<Uuid>,
    pub mode: AppMode,
    pub login_sid: Uuid,
    /// The apps-origin grant this token was minted under (spec §6.2); `None` for a
    /// token minted outside the proxy.
    pub grant_id: Option<Uuid>,
    /// The real admin, when the user was being impersonated at mint time. Carried
    /// into every gateway audit event so an impersonated query is never recorded as
    /// the user's own.
    pub impersonator: Option<UserId>,
}

#[derive(Debug, Clone)]
pub struct AppSession {
    pub id: Uuid,
    pub user_id: UserId,
    pub workspace_id: WorkspaceId,
    pub app_id: Uuid,
    pub version_id: Option<Uuid>,
    pub mode: AppMode,
    pub impersonator: Option<UserId>,
}

#[derive(Clone)]
pub struct AppSessionRepo {
    db: Db,
}

impl AppSessionRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Returns the plaintext token exactly once; only its hash is stored.
    pub async fn mint(
        &self,
        new: &NewAppSession,
        idle: Duration,
        absolute: Duration,
    ) -> egret_core::Result<String> {
        let plaintext = format!("{TOKEN_PREFIX}{}", token::random_token_b64());
        sqlx::query(
            "INSERT INTO analytics.app_sessions
               (token_hash, user_id, workspace_id, app_id, version_id, mode, login_sid,
                grant_id, impersonator_user_id, idle_expires_at, absolute_expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9,
                     now() + make_interval(secs => $10), now() + make_interval(secs => $11))",
        )
        .bind(token::hash_token(&plaintext))
        .bind(new.user_id.0)
        .bind(new.workspace_id.0)
        .bind(new.app_id)
        .bind(new.version_id)
        .bind(new.mode.as_str())
        .bind(new.login_sid)
        .bind(new.grant_id)
        .bind(new.impersonator.map(|u| u.0))
        .bind(idle.as_secs_f64())
        .bind(absolute.as_secs_f64())
        .execute(&self.db.pool)
        .await
        .map_err(anyhow::Error::from)?;
        Ok(plaintext)
    }

    /// Looks up by hash and, if live, slides the idle expiry in the same statement
    /// (clamped to the absolute expiry).
    ///
    /// Live includes the LOGIN session: a row of `sessions` whose idle or absolute
    /// window has lapsed but which no prune has deleted yet no longer carries the
    /// app tokens minted under it. The login session's own idle window is NOT slid
    /// here — a person querying from a pod is not browsing egret.
    pub async fn resolve(
        &self,
        token_plain: &str,
        idle: Duration,
    ) -> egret_core::Result<Option<AppSession>> {
        if !token_plain.starts_with(TOKEN_PREFIX) {
            return Ok(None);
        }
        let row = sqlx::query(
            "UPDATE analytics.app_sessions
                SET last_used_at = now(),
                    idle_expires_at = LEAST(now() + make_interval(secs => $2), absolute_expires_at)
              WHERE token_hash = $1
                AND revoked_at IS NULL
                AND idle_expires_at > now()
                AND absolute_expires_at > now()
                AND EXISTS (SELECT 1 FROM sessions s
                             WHERE s.id = app_sessions.login_sid
                               AND s.idle_expires_at > now()
                               AND s.absolute_expires_at > now())
          RETURNING id, user_id, workspace_id, app_id, version_id, mode, impersonator_user_id",
        )
        .bind(token::hash_token(token_plain))
        .bind(idle.as_secs_f64())
        .fetch_optional(&self.db.pool)
        .await
        .map_err(anyhow::Error::from)?;
        row.map(|r| {
            Ok(AppSession {
                id: r.get("id"),
                user_id: UserId(r.get("user_id")),
                workspace_id: WorkspaceId(r.get("workspace_id")),
                app_id: r.get("app_id"),
                version_id: r.get("version_id"),
                mode: AppMode::parse(r.get::<&str, _>("mode"))?,
                impersonator: r.get::<Option<Uuid>, _>("impersonator_user_id").map(UserId),
            })
        })
        .transpose()
    }

    pub async fn revoke(&self, token_plain: &str) -> egret_core::Result<()> {
        sqlx::query(
            "UPDATE analytics.app_sessions SET revoked_at = now() WHERE token_hash = $1 AND revoked_at IS NULL",
        )
        .bind(token::hash_token(token_plain))
        .execute(&self.db.pool)
        .await
        .map_err(anyhow::Error::from)?;
        Ok(())
    }

    pub async fn revoke_by_id(&self, id: Uuid) -> egret_core::Result<()> {
        sqlx::query(
            "UPDATE analytics.app_sessions SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL",
        )
        .bind(id)
        .execute(&self.db.pool)
        .await
        .map_err(anyhow::Error::from)?;
        Ok(())
    }

    /// Every live token minted under one egret login session. Returns how many it
    /// revoked. Logout deletes the login session and cascades; this is for a caller
    /// that wants the tokens dead while the login session lives on.
    pub async fn revoke_for_login_session(&self, login_sid: Uuid) -> egret_core::Result<u64> {
        let r = sqlx::query(
            "UPDATE analytics.app_sessions SET revoked_at = now() WHERE login_sid = $1 AND revoked_at IS NULL",
        )
        .bind(login_sid)
        .execute(&self.db.pool)
        .await
        .map_err(anyhow::Error::from)?;
        Ok(r.rows_affected())
    }

    /// Every live token minted under one apps-origin grant (the proxy calls this when
    /// the grant's last WebSocket closes). Returns how many it revoked.
    pub async fn revoke_for_grant(&self, grant_id: Uuid) -> egret_core::Result<u64> {
        let r = sqlx::query(
            "UPDATE analytics.app_sessions SET revoked_at = now() WHERE grant_id = $1 AND revoked_at IS NULL",
        )
        .bind(grant_id)
        .execute(&self.db.pool)
        .await
        .map_err(anyhow::Error::from)?;
        Ok(r.rows_affected())
    }

    /// Deletes rows that have been dead — revoked, idle-expired or absolute-expired —
    /// for longer than `older_than`. A recently dead row stays: it is the record that
    /// explains a 401 someone is still debugging.
    pub async fn sweep_expired(&self, older_than: Duration) -> egret_core::Result<u64> {
        let r = sqlx::query(
            "DELETE FROM analytics.app_sessions \
              WHERE COALESCE(revoked_at, 'infinity'::timestamptz) < now() - make_interval(secs => $1) \
                 OR idle_expires_at < now() - make_interval(secs => $1) \
                 OR absolute_expires_at < now() - make_interval(secs => $1)",
        )
        .bind(older_than.as_secs_f64())
        .execute(&self.db.pool)
        .await
        .map_err(anyhow::Error::from)?;
        Ok(r.rows_affected())
    }
}
