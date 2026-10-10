//! Apps-origin grants (spec §6.1): a one-time ticket, issued on the egret origin,
//! that becomes an apps-origin cookie.
//!
//! A grant is ONE row, `analytics.app_grants`. Its ticket is spent by setting
//! `redeemed_at` in a single `UPDATE … WHERE redeemed_at IS NULL`, so two racing
//! redeems cannot both win and a replay is a plain zero-row update. The cookie that
//! comes out of it carries a plaintext whose hash is the row's `cookie_hash`.
//!
//! The row's `login_sid` cascades from egret's own login session, so logout (and
//! OIDC back-channel logout) removes the grant without any code remembering to; an
//! expired-but-undeleted login session is refused here too, as in
//! [`crate::session::AppSessionRepo::resolve`].

use std::time::Duration;

use egret_core::{token, Result, UserId, WorkspaceId};
use egret_metastore::Db;
use sqlx::Row;
use uuid::Uuid;

use crate::session::AppMode;

pub const TICKET_PREFIX: &str = "egt_";
pub const COOKIE_PREFIX: &str = "egc_";

#[derive(Debug, Clone)]
pub struct NewGrant {
    pub user_id: UserId,
    pub workspace_id: WorkspaceId,
    pub app_id: Uuid,
    /// `None` exactly when `mode` is `Edit` (enforced by a CHECK).
    pub version_id: Option<Uuid>,
    pub mode: AppMode,
    pub login_sid: Uuid,
    pub impersonator: Option<UserId>,
}

#[derive(Debug, Clone)]
pub struct Grant {
    pub id: Uuid,
    pub user_id: UserId,
    pub workspace_id: WorkspaceId,
    pub app_id: Uuid,
    pub version_id: Option<Uuid>,
    pub mode: AppMode,
    pub login_sid: Uuid,
    pub impersonator: Option<UserId>,
}

#[derive(Clone)]
pub struct GrantRepo {
    db: Db,
}

fn db_err(e: sqlx::Error) -> egret_core::Error {
    anyhow::Error::from(e).into()
}

fn grant_of(r: &sqlx::postgres::PgRow) -> Result<Grant> {
    let mode = match r.get::<&str, _>("mode") {
        "edit" => AppMode::Edit,
        "run" => AppMode::Run,
        other => return Err(anyhow::anyhow!("unknown grant mode {other}").into()),
    };
    Ok(Grant {
        id: r.get("id"),
        user_id: UserId(r.get("user_id")),
        workspace_id: WorkspaceId(r.get("workspace_id")),
        app_id: r.get("app_id"),
        version_id: r.get("version_id"),
        mode,
        login_sid: r.get("login_sid"),
        impersonator: r.get::<Option<Uuid>, _>("impersonator_user_id").map(UserId),
    })
}

impl GrantRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Returns the ticket plaintext exactly once; only its hash is stored.
    pub async fn issue_ticket(&self, new: &NewGrant, ttl: Duration) -> Result<String> {
        let plaintext = format!("{TICKET_PREFIX}{}", token::random_token_b64());
        sqlx::query(
            "INSERT INTO analytics.app_grants \
               (ticket_hash, user_id, workspace_id, app_id, version_id, mode, login_sid, \
                impersonator_user_id, ticket_expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now() + make_interval(secs => $9))",
        )
        .bind(token::hash_token(&plaintext))
        .bind(new.user_id.0)
        .bind(new.workspace_id.0)
        .bind(new.app_id)
        .bind(new.version_id)
        .bind(new.mode.as_str())
        .bind(new.login_sid)
        .bind(new.impersonator.map(|u| u.0))
        .bind(ttl.as_secs_f64())
        .execute(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(plaintext)
    }

    /// Spends the ticket and returns the grant with the cookie plaintext (once).
    /// `None` for an unknown, spent, expired or revoked ticket, a login session that
    /// has lapsed, or a user who is no longer active. One statement: concurrent
    /// redeems of one ticket produce exactly one `Some`.
    pub async fn redeem(
        &self,
        ticket: &str,
        cookie_ttl: Duration,
    ) -> Result<Option<(Grant, String)>> {
        if !ticket.starts_with(TICKET_PREFIX) {
            return Ok(None);
        }
        let cookie = format!("{COOKIE_PREFIX}{}", token::random_token_b64());
        let row = sqlx::query(
            "UPDATE analytics.app_grants g \
                SET cookie_hash = $2, redeemed_at = now(), \
                    cookie_expires_at = now() + make_interval(secs => $3) \
              WHERE g.ticket_hash = $1 \
                AND g.redeemed_at IS NULL \
                AND g.revoked_at IS NULL \
                AND g.ticket_expires_at > now() \
                AND EXISTS (SELECT 1 FROM sessions s \
                             WHERE s.id = g.login_sid \
                               AND s.idle_expires_at > now() \
                               AND s.absolute_expires_at > now()) \
                AND EXISTS (SELECT 1 FROM users u WHERE u.id = g.user_id AND u.status = 'active') \
          RETURNING id, user_id, workspace_id, app_id, version_id, mode, login_sid, \
                    impersonator_user_id",
        )
        .bind(token::hash_token(ticket))
        .bind(token::hash_token(&cookie))
        .bind(cookie_ttl.as_secs_f64())
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        row.map(|r| Ok((grant_of(&r)?, cookie))).transpose()
    }

    /// The grant behind an apps cookie, if it is still good: redeemed, not revoked,
    /// inside its own expiry, its login session unexpired, its user active.
    pub async fn resolve_cookie(&self, cookie: &str) -> Result<Option<Grant>> {
        if !cookie.starts_with(COOKIE_PREFIX) {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT id, user_id, workspace_id, app_id, version_id, mode, login_sid, \
                    impersonator_user_id \
               FROM analytics.app_grants g \
              WHERE g.cookie_hash = $1 \
                AND g.revoked_at IS NULL \
                AND g.cookie_expires_at > now() \
                AND EXISTS (SELECT 1 FROM sessions s \
                             WHERE s.id = g.login_sid \
                               AND s.idle_expires_at > now() \
                               AND s.absolute_expires_at > now()) \
                AND EXISTS (SELECT 1 FROM users u WHERE u.id = g.user_id AND u.status = 'active')",
        )
        .bind(token::hash_token(cookie))
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        row.map(|r| grant_of(&r)).transpose()
    }

    pub async fn revoke(&self, grant_id: Uuid) -> Result<()> {
        sqlx::query(
            "UPDATE analytics.app_grants SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL",
        )
        .bind(grant_id)
        .execute(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    /// Deletes grants that have been dead for longer than `older_than`: a ticket that
    /// was never redeemed and expired, a cookie that expired, or a revoked grant. The
    /// app sessions minted under a deleted grant go with it (`ON DELETE CASCADE`).
    pub async fn sweep(&self, older_than: Duration) -> Result<u64> {
        let r = sqlx::query(
            "DELETE FROM analytics.app_grants \
              WHERE (redeemed_at IS NULL \
                     AND ticket_expires_at < now() - make_interval(secs => $1)) \
                 OR cookie_expires_at < now() - make_interval(secs => $1) \
                 OR revoked_at < now() - make_interval(secs => $1)",
        )
        .bind(older_than.as_secs_f64())
        .execute(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(r.rows_affected())
    }
}
