//! Platform-admin audit-log query API (read-only; append-only store).
use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use egret_auth::principal::Auth;

use crate::{CpError, CpState};

#[derive(Debug, Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct AuditQueryParams {
    pub actor: Option<uuid::Uuid>,
    pub action: Option<String>,
    pub kind: Option<String>,
    pub decision: Option<String>,
    /// `true` leaves out authorization decisions that allowed the call
    /// (routine reads): what is left is changes and denials.
    pub hide_allowed: Option<bool>,
    pub workspace: Option<uuid::Uuid>,
    pub from: Option<chrono::DateTime<chrono::Utc>>,
    pub to: Option<chrono::DateTime<chrono::Utc>>,
    /// Keyset cursor from a prior page's `nextCursor`.
    pub cursor: Option<String>, // "<ts_rfc3339>,<id>"
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntryDto {
    pub id: i64,
    pub ts: chrono::DateTime<chrono::Utc>,
    pub kind: String,
    pub actor_user_id: Option<uuid::Uuid>,
    /// The actor's email, when the actor is a user who still exists.
    pub actor_email: Option<String>,
    pub workspace_id: Option<uuid::Uuid>,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Option<String>,
    pub decision: Option<String>,
    pub reasons: Vec<String>,
    pub detail: serde_json::Value,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditPage {
    pub entries: Vec<AuditEntryDto>,
    pub next_cursor: Option<String>,
}

pub fn audit_router() -> Router<CpState> {
    // Route is RELATIVE to the `/api/control-plane` nest in `egret-server`
    // (like every sibling `access::router()` route, e.g. `/users`). A leading
    // `/control-plane/…` here would double-prefix to `/api/control-plane/
    // control-plane/audit` — which 404s the FE (it calls `/api/control-plane/
    // audit`, matching the `#[utoipa::path]` below).
    Router::new().route("/audit", get(list_audit))
}

fn parse_cursor(s: &str) -> Option<(chrono::DateTime<chrono::Utc>, i64)> {
    let (ts, id) = s.split_once(',')?;
    Some((ts.parse().ok()?, id.parse().ok()?))
}

#[utoipa::path(
    get, path = "/api/control-plane/audit", tag = "control-plane-audit",
    params(AuditQueryParams),
    responses(
        (status = 200, description = "Audit entries (newest first)", body = AuditPage),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Requires platform_admin", body = egret_core::http::ErrorBody),
    ),
)]
async fn list_audit(
    State(st): State<CpState>,
    Auth(p): Auth,
    Query(q): Query<AuditQueryParams>,
) -> Result<Json<AuditPage>, CpError> {
    // The audit trail is platform-admin-only. `audit:read` is an ADMIN_GRAPH action
    // (not in the grantable catalog), so only platform_admin passes — a non-admin
    // governance role like `data_steward` (which holds `role:read`) is denied.
    egret_core::authz::require(&*st.pdp, &p, "audit:read").await?;
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let query = egret_audit::AuditQuery {
        actor: q.actor,
        action: q.action,
        kind: q.kind,
        decision: q.decision,
        hide_allowed: q.hide_allowed.unwrap_or(false),
        workspace: q.workspace,
        from: q.from,
        to: q.to,
        before: q.cursor.as_deref().and_then(parse_cursor),
        limit,
    };
    let rows = st.audit_store.query(&query).await.map_err(|e| {
        tracing::warn!(error = %e, "audit query failed");
        CpError::Internal
    })?;
    let next_cursor = if rows.len() as i64 == limit {
        rows.last()
            .map(|r| format!("{},{}", r.ts.to_rfc3339(), r.id))
    } else {
        None
    };
    let entries = rows
        .into_iter()
        .map(|r| AuditEntryDto {
            id: r.id,
            ts: r.ts,
            kind: r.kind,
            actor_user_id: r.actor_user_id,
            actor_email: r.actor_email,
            workspace_id: r.workspace_id,
            action: r.action,
            resource_type: r.resource_type,
            resource_id: r.resource_id,
            decision: r.decision,
            reasons: r.reasons.unwrap_or_default(),
            detail: r.detail,
        })
        .collect();
    Ok(Json(AuditPage {
        entries,
        next_cursor,
    }))
}

/// OpenAPI document for the audit query API; merged by `crate::openapi()`.
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(list_audit),
    components(schemas(AuditQueryParams, AuditEntryDto, AuditPage)),
    tags((name = "control-plane-audit", description = "Platform audit-log query (read-only, append-only store)")),
)]
pub struct ControlPlaneAuditApi;
