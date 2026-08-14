//! The impersonation overlay store. An overlay row makes `SessionService::validate`
//! resolve an admin's session to the target user's `Principal` (see session.rs),
//! while remembering the admin. At most one active row per admin session.

use egret_core::{Error, Result, UserId, WorkspaceId};
use egret_metastore::Db;
use uuid::Uuid;

use crate::session::SessionService;
use crate::users::UserRepo;

/// Validate that `impersonator` (already known to be an authorized admin for the
/// intended scope) may target `target_user_id`. Enforces the target-facing
/// invariants shared by **every** impersonation entry point: not self, reason
/// non-empty, target exists, the target is not a service account (a machine
/// principal must never end up inside an interactive session), and the target is
/// not itself a `platform_admin` (admins are off-limits so an overlay can never
/// be used to borrow admin power).
///
/// It deliberately does **not** check the caller's own authority (platform-admin
/// vs ws-admin) — that differs per entry point and the caller enforces it first,
/// before reaching here. Returns `Ok(())` or the domain [`Error`] that maps to the
/// right HTTP status (`InvalidInput` → 400, `Forbidden` → 403).
///
/// Used by the platform start endpoint (Task 7), the OIDC step-up callback
/// (Task 8), and the ws_admin start endpoint (Task 9) so the guard set stays
/// identical across all three.
pub async fn validate_impersonation_target(
    users: &UserRepo,
    sessions: &SessionService,
    impersonator_id: UserId,
    target_user_id: UserId,
    reason: &str,
) -> Result<()> {
    if target_user_id == impersonator_id {
        return Err(Error::InvalidInput("cannot impersonate yourself".into()));
    }
    if reason.trim().is_empty() {
        return Err(Error::InvalidInput("reason is required".into()));
    }
    // Target must exist.
    let target = users
        .find_by_id(target_user_id)
        .await?
        .ok_or_else(|| Error::InvalidInput("unknown target".into()))?;
    // Target must be a PERSON. A service account (Data API P2) can never hold an
    // interactive session — migration 0119's `users_service_has_no_password`
    // CHECK exists to make that structural — and an overlay is exactly the way
    // to hand one a session anyway, since it resolves an admin's real session to
    // the target's `Principal`. It would also make the audit trail read as a
    // human acting as a machine, which is the reverse of what impersonation is
    // for. To act as a service account, use its OAuth2 client.
    if target.provider == "service" {
        return Err(Error::InvalidInput(
            "cannot impersonate a service account".into(),
        ));
    }
    // Target must NOT itself be a platform admin.
    let target_roles = sessions.platform_roles_of(target_user_id).await?;
    if target_roles.iter().any(|r| r == "platform_admin") {
        return Err(Error::Forbidden);
    }
    Ok(())
}

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
    /// ACTIVE overlay on the same session (→ `Conflict`), which enforces no-nesting.
    /// A stale, already-expired overlay for this session is reaped first (in the
    /// same transaction as the insert) so a dead row never blocks a fresh start —
    /// `start` doesn't depend on `active_for_session` having run first to clean it up.
    pub async fn start(&self, n: NewImpersonation) -> Result<()> {
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        // Reap a dead overlay for this session so a stale expired row doesn't block
        // a fresh start. This deletes ONLY expired rows — an ACTIVE overlay is left
        // untouched, so the INSERT below still hits the unique index and returns
        // Conflict, preserving no-nesting.
        sqlx::query(
            "DELETE FROM impersonations WHERE admin_session_id = $1 AND expires_at <= now()",
        )
        .bind(n.admin_session_id)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
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
        .execute(&mut *tx)
        .await;
        // Map the unique-violation on an ACTIVE row to Conflict, same as before.
        match res {
            Ok(_) => {
                tx.commit().await.map_err(db_err)?;
                Ok(())
            }
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
