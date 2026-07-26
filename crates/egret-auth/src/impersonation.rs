//! The impersonation overlay store. An overlay row makes `SessionService::validate`
//! resolve an admin's session to the target user's `Principal` (see session.rs),
//! while remembering the admin. At most one active row per admin session.

use egret_core::{Error, Result, UserId, WorkspaceId};
use egret_metastore::Db;
use uuid::Uuid;

#[derive(Clone)]
pub struct ImpersonationRepo {
    db: Db,
}

pub struct NewImpersonation {
    pub admin_session_id: Uuid,
    pub impersonator_id: UserId,
    pub target_user_id: UserId,
    pub workspace_scope: Option<WorkspaceId>,
    pub reason: String,
    pub ttl_minutes: i64,
}

/// A currently-active (non-expired) overlay, as read by `validate`.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ActiveImpersonation {
    pub impersonator_id: Uuid,
    pub target_user_id: Uuid,
    pub workspace_scope_id: Option<Uuid>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::anyhow!(e))
}

impl ImpersonationRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Insert an overlay. The unique index on `admin_session_id` rejects a second
    /// active overlay on the same session (→ `Conflict`), which enforces no-nesting.
    pub async fn start(&self, n: NewImpersonation) -> Result<()> {
        let res = sqlx::query(
            "INSERT INTO impersonations \
             (admin_session_id, impersonator_id, target_user_id, workspace_scope_id, reason, expires_at) \
             VALUES ($1, $2, $3, $4, $5, now() + make_interval(mins => $6::int))",
        )
        .bind(n.admin_session_id)
        .bind(n.impersonator_id.0)
        .bind(n.target_user_id.0)
        .bind(n.workspace_scope.map(|w| w.0))
        .bind(&n.reason)
        .bind(n.ttl_minutes)
        .execute(&self.db.pool)
        .await;
        match res {
            Ok(_) => Ok(()),
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Err(Error::Conflict(
                "session already has an active impersonation overlay".into(),
            )),
            Err(e) => Err(db_err(e)),
        }
    }

    /// The active (non-expired) overlay for `admin_session_id`, if any. Expired
    /// rows are treated as absent AND best-effort deleted (the auto-return path).
    pub async fn active_for_session(
        &self,
        admin_session_id: Uuid,
    ) -> Result<Option<ActiveImpersonation>> {
        let row = sqlx::query_as::<_, ActiveImpersonation>(
            "SELECT impersonator_id, target_user_id, workspace_scope_id, expires_at \
             FROM impersonations WHERE admin_session_id = $1 AND expires_at > now()",
        )
        .bind(admin_session_id)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;

        if row.is_none() {
            let _ = sqlx::query(
                "DELETE FROM impersonations WHERE admin_session_id = $1 AND expires_at <= now()",
            )
            .bind(admin_session_id)
            .execute(&self.db.pool)
            .await;
        }
        Ok(row)
    }

    pub async fn exit(&self, admin_session_id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM impersonations WHERE admin_session_id = $1")
            .bind(admin_session_id)
            .execute(&self.db.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }
}
