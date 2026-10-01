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
/// principal must never end up inside an interactive session), and a
/// platform-authority rule that depends on `intended_scope` — see the ⚠ paragraph
/// at that check.
///
/// `intended_scope` is the `workspace_scope` the overlay about to be written will
/// carry: `Some(ws)` for the ws_admin entry point, `None` for the platform one.
/// It is **not** the caller's authority — this function still does not check that,
/// because it differs per entry point and the caller enforces it first, before
/// reaching here. It is the *reach* of the overlay being created, which is what
/// decides how much the target may hold.
///
/// Returns `Ok(())` or the domain [`Error`] that maps to the right HTTP status
/// (`InvalidInput` → 400, `Forbidden` → 403).
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
    intended_scope: Option<WorkspaceId>,
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
    // Platform authority, by the REACH of the overlay about to be written.
    //
    // * A **ws-scoped** overlay (`intended_scope = Some(ws)`) may target nobody
    //   holding any platform-scoped role grant at all.
    // * A **platform-scoped** overlay (`None`) keeps the original narrower rule:
    //   not a `platform_admin`.
    //
    // ⚠ **This used to read `.any(|r| r == "platform_admin")`, and that was a
    // privilege escalation with a measured path, SP4d debt Task 13.** The chain,
    // by code path rather than by intent:
    //
    // * `ws_impersonate_start` (`egret_workspace::routes`) lets a `ws_admin` of
    //   workspace A target any MEMBER of A who is not a `ws_admin` there, and the
    //   overlay it writes carries `workspace_scope = Some(A)`.
    // * `egret_workspace::context::WsContext` confines a scoped overlay to that
    //   one workspace — but only on a route that resolves a `{workspace}` path
    //   segment, which is the only thing that builds a `WsContext`. **No
    //   `/api/control-plane` route resolves one**, so none is confined there;
    //   measured, every handler on `control_plane_router`,
    //   `policy_admin_router`, `catalog_admin_router`, `engine_mgmt_router` and
    //   `perm_router` gates on a `Scope::Platform` permission instead.
    // * Migration `0059_rbac_permission_seed.sql` seeds the platform
    //   `data_steward` role with the whole `policy:*` family — `policy:update`
    //   included.
    //
    // ⇒ a `ws_admin` of A impersonated a member of A holding `data_steward` and
    // then *created a permission grant on a catalog of workspace B*, which
    // compiles into the Ranger artifact B's engine downloads. Refusing the target
    // here removes the escalation at its source, for every platform route at once,
    // instead of at each consumer.
    //
    // **Why the rule is keyed to `intended_scope` and not applied to both arms.**
    // The first version of this fix applied the wide rule unconditionally, and that
    // was wrong on the platform path: the caller there is already a
    // `platform_admin`, whose PDP bypass means they hold strictly more than any
    // `data_steward`, so refusing that target prevents no escalation and removes a
    // real support path (reproducing what a steward sees). The escalation this
    // guard exists for is a **ws-scoped** overlay acquiring platform authority that
    // no `/api/control-plane` route confines — so the condition is the overlay's
    // reach, which is the thing that differs, rather than a second predicate
    // somewhere else or a wide rule that costs the platform path for nothing.
    //
    // ⚠ **The permissive arm is load-bearing and is asserted in both directions**
    // (`a_target_holding_any_platform_role_grant_is_refused`, which drives all four
    // cells of {scope, no scope} × {plain target, platform-role target}). A future
    // tightening that collapsed the `match` to the wide rule would be invisible
    // without it: nothing else in the suite notices a guard that merely refuses
    // more.
    //
    // **Why ANY platform role and not "any platform role carrying a
    // permission".** `platform_roles` is the same query that populates
    // `Principal::platform_roles` for the overlay (see `SessionService::validate`,
    // which calls it for the TARGET), and `egret_auth::grants::grants_for` admits
    // only `scope_type = 'platform'` rows when the PDP decides a
    // `Resource::Platform` — so a platform-scoped role grant is the *only* way the
    // overlay principal can satisfy any of those gates, and this emptiness test
    // cannot disagree with the authority the overlay would confer. Narrowing it to
    // `role_permissions` being non-empty would have been a second spelling of the
    // PDP's own role closure and could fail open the day the two diverged. The
    // cost of the wider form is refusing a target whose only platform grant is to
    // a role that carries nothing, which grants nothing and so loses nothing.
    //
    // It is deliberately stricter than it needs to be for the *platform* entry
    // point, where the caller is already a `platform_admin` and so gains nothing
    // from the target's roles. Keeping one target rule for all three entry points
    // is the shape the rest of this function already has, and a per-entry-point
    // parameter would be the second predicate this guard exists to avoid.
    let target_roles = sessions.platform_roles_of(target_user_id).await?;
    let refuse = match intended_scope {
        Some(_) => !target_roles.is_empty(),
        None => target_roles.iter().any(|r| r == "platform_admin"),
    };
    if refuse {
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
