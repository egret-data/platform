//! Access-request → approval workflow (P2b), data layer.
//!
//! A user asks to JOIN a workspace (`create`); a workspace admin `approve`s
//! (which adds them as a member with an optional time-boxed expiry via
//! [`MemberRepo`], the single grant path) or `deny`s. The requester may
//! `cancel` their own still-pending request. `joinable` lists the active
//! workspaces a user is NOT already a member of (the request targets).
//!
//! One pending request per (requester, workspace) is enforced by a partial
//! unique index (mig 0079); a duplicate surfaces as [`Error::Conflict`].

use egret_core::{Error, Result, UserId, WorkspaceId};
use egret_metastore::Db;
use uuid::Uuid;

use crate::members::MemberRepo;
use crate::workspaces::db_err;

#[derive(Clone)]
pub struct AccessRequestRepo {
    db: Db,
    /// The single member-grant path. Approval MUST route through this (no second
    /// grant path) so the one-role-per-member + expiry invariants hold.
    members: MemberRepo,
}

/// A join request row.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AccessRequest {
    pub id: Uuid,
    pub requester_id: Uuid,
    pub workspace_id: Uuid,
    pub requested_role: String,
    pub reason: String,
    pub status: String,
    pub decided_by: Option<Uuid>,
    pub decided_at: Option<chrono::DateTime<chrono::Utc>>,
    pub decision_note: Option<String>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// A join request enriched for list views with the requester's display name and
/// the workspace's name (JOINed from `users` / `workspaces`).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AccessRequestEnriched {
    pub id: Uuid,
    pub requester_id: Uuid,
    pub workspace_id: Uuid,
    pub requested_role: String,
    pub reason: String,
    pub status: String,
    pub decided_by: Option<Uuid>,
    pub decided_at: Option<chrono::DateTime<chrono::Utc>>,
    pub decision_note: Option<String>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub requester_name: Option<String>,
    pub workspace_name: Option<String>,
}

const REQUEST_COLS: &str = "id, requester_id, workspace_id, requested_role, reason, status, \
     decided_by, decided_at, decision_note, expires_at, created_at";

impl AccessRequestRepo {
    pub fn new(db: Db, members: MemberRepo) -> Self {
        Self { db, members }
    }

    /// Insert a `pending` join request. A second pending request for the same
    /// (requester, workspace) violates the partial unique index → [`Error::Conflict`].
    pub async fn create(
        &self,
        requester_id: UserId,
        workspace_id: WorkspaceId,
        role: &str,
        reason: &str,
    ) -> Result<AccessRequest> {
        sqlx::query_as::<_, AccessRequest>(sqlx::AssertSqlSafe(format!(
            "INSERT INTO access_requests (requester_id, workspace_id, requested_role, reason) \
             VALUES ($1, $2, $3, $4) RETURNING {REQUEST_COLS}"
        )))
        .bind(requester_id.0)
        .bind(workspace_id.0)
        .bind(role)
        .bind(reason)
        .fetch_one(&self.db.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::Database(db) if db.is_unique_violation() => {
                Error::Conflict("a pending request for this workspace already exists".into())
            }
            other => db_err(other),
        })
    }

    /// A user's own requests, newest first (enriched with workspace name).
    pub async fn list_for_requester(&self, user_id: UserId) -> Result<Vec<AccessRequestEnriched>> {
        sqlx::query_as::<_, AccessRequestEnriched>(
            "SELECT ar.id, ar.requester_id, ar.workspace_id, ar.requested_role, ar.reason, \
                    ar.status, ar.decided_by, ar.decided_at, ar.decision_note, ar.expires_at, \
                    ar.created_at, u.email::text AS requester_name, w.name AS workspace_name \
             FROM access_requests ar \
             LEFT JOIN users u ON u.id = ar.requester_id \
             LEFT JOIN workspaces w ON w.id = ar.workspace_id \
             WHERE ar.requester_id = $1 \
             ORDER BY ar.created_at DESC",
        )
        .bind(user_id.0)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)
    }

    /// A workspace's requests filtered by `status`, newest first (enriched with
    /// the requester's email as display name).
    pub async fn list_for_workspace(
        &self,
        ws: WorkspaceId,
        status: &str,
    ) -> Result<Vec<AccessRequestEnriched>> {
        sqlx::query_as::<_, AccessRequestEnriched>(
            "SELECT ar.id, ar.requester_id, ar.workspace_id, ar.requested_role, ar.reason, \
                    ar.status, ar.decided_by, ar.decided_at, ar.decision_note, ar.expires_at, \
                    ar.created_at, u.email::text AS requester_name, w.name AS workspace_name \
             FROM access_requests ar \
             LEFT JOIN users u ON u.id = ar.requester_id \
             LEFT JOIN workspaces w ON w.id = ar.workspace_id \
             WHERE ar.workspace_id = $1 AND ar.status = $2 \
             ORDER BY ar.created_at DESC",
        )
        .bind(ws.0)
        .bind(status)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)
    }

    /// Fetch a single request by id.
    pub async fn get(&self, id: Uuid) -> Result<Option<AccessRequest>> {
        sqlx::query_as::<_, AccessRequest>(sqlx::AssertSqlSafe(format!(
            "SELECT {REQUEST_COLS} FROM access_requests WHERE id = $1"
        )))
        .bind(id)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)
    }

    /// Approve a pending request: add the requester as a workspace member with
    /// the chosen role (defaulting to the requested one) and optional expiry via
    /// the single [`MemberRepo`] grant path, then mark the request approved.
    ///
    /// The request row is locked `FOR UPDATE` for the duration; the request must
    /// belong to `ws` (else [`Error::NotFound`] — cross-workspace guard) and be
    /// `pending` (else [`Error::Conflict`]).
    pub async fn approve(
        &self,
        id: Uuid,
        ws: WorkspaceId,
        role: Option<&str>,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
        by: UserId,
    ) -> Result<()> {
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        let row = sqlx::query_as::<_, AccessRequest>(sqlx::AssertSqlSafe(format!(
            "SELECT {REQUEST_COLS} FROM access_requests WHERE id = $1 FOR UPDATE"
        )))
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?
        .ok_or_else(|| Error::NotFound("access request".into()))?;

        if row.workspace_id != ws.0 {
            return Err(Error::NotFound("access request".into()));
        }
        if row.status != "pending" {
            return Err(Error::Conflict(format!(
                "request is already {}",
                row.status
            )));
        }

        // Single grant path: add the member with the chosen role + expiry.
        let role = role.unwrap_or(row.requested_role.as_str());
        self.members
            .add_with_expiry(ws, "user", row.requester_id, role, by, expires_at)
            .await?;

        sqlx::query(
            "UPDATE access_requests \
             SET status = 'approved', decided_by = $2, decided_at = now(), expires_at = $3 \
             WHERE id = $1",
        )
        .bind(id)
        .bind(by.0)
        .bind(expires_at)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }

    /// Deny a pending request with an optional note. Must belong to `ws`
    /// ([`Error::NotFound`]) and be `pending` ([`Error::Conflict`]).
    pub async fn deny(
        &self,
        id: Uuid,
        ws: WorkspaceId,
        note: Option<&str>,
        by: UserId,
    ) -> Result<()> {
        let n = sqlx::query(
            "UPDATE access_requests \
             SET status = 'denied', decided_by = $3, decided_at = now(), decision_note = $4 \
             WHERE id = $1 AND workspace_id = $2 AND status = 'pending'",
        )
        .bind(id)
        .bind(ws.0)
        .bind(by.0)
        .bind(note)
        .execute(&self.db.pool)
        .await
        .map_err(db_err)?
        .rows_affected();
        if n == 0 {
            // Either not found in this workspace, or no longer pending.
            return Err(Error::Conflict(
                "request is not pending or does not belong to this workspace".into(),
            ));
        }
        Ok(())
    }

    /// Cancel one's OWN still-pending request. A cancel by a different user or of
    /// a non-pending request affects no rows → [`Error::NotFound`].
    pub async fn cancel(&self, id: Uuid, requester_id: UserId) -> Result<()> {
        let n = sqlx::query(
            "UPDATE access_requests SET status = 'cancelled' \
             WHERE id = $1 AND requester_id = $2 AND status = 'pending'",
        )
        .bind(id)
        .bind(requester_id.0)
        .execute(&self.db.pool)
        .await
        .map_err(db_err)?
        .rows_affected();
        if n == 0 {
            return Err(Error::NotFound(
                "no pending request to cancel for this user".into(),
            ));
        }
        Ok(())
    }

    /// Active workspaces the user is NOT already a member of (directly or via a
    /// group) — the valid targets for a join request. Mirrors
    /// `WorkspaceRepo::list_for_principal`'s grant subquery, including the P2a
    /// expiry filter. Returns `(id, slug, name)` ordered by name.
    pub async fn joinable(&self, user_id: UserId) -> Result<Vec<(Uuid, String, String)>> {
        let rows = sqlx::query_as::<_, (Uuid, String, String)>(
            "SELECT w.id, w.slug, w.name FROM workspaces w \
             WHERE w.status = 'active' \
               AND NOT EXISTS ( \
                 SELECT 1 FROM access_grants ag \
                 WHERE ag.scope_id = w.id AND ag.scope_type = 'workspace' \
                   AND (ag.expires_at IS NULL OR ag.expires_at > now()) \
                   AND ( (ag.principal_type = 'user'  AND ag.principal_id = $1) \
                      OR (ag.principal_type = 'group' AND ag.principal_id IN \
                             (SELECT group_id FROM user_groups WHERE user_id = $1)) ) \
               ) \
             ORDER BY w.name",
        )
        .bind(user_id.0)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows)
    }
}
