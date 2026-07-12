//! Object-level share API (Effort B / B2-1a Task 4). Per-object grants let a
//! `<kind>:manage` holder share a single dashboard / chart / dataset / folder
//! with an outside user or group at a workspace role ("Viewer" = `ws_viewer`,
//! "Editor" = `ws_editor`). The grants are rows in `access_grants` with
//! `scope_type = 'object'`; the PDP (`grants_for` + `CedarPdp`) resolves them,
//! and a folder grant cascades to the items inside it.

use axum::extract::{Path, State};
use axum::routing::{delete, put};
use axum::{Json, Router};
use egret_core::authz::{require_object, ObjectRef};
use egret_core::{Error, WorkspaceContext, WorkspacePath};
use egret_metastore::Db;
use egret_workspace::context::WsContext;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::BiApiError;
use crate::routes::BiState;

/// The object kinds a share can target and their backing table under `analytics`.
/// Hard-coded so `kind` never reaches SQL as an interpolated identifier.
fn object_table(kind: &str) -> Option<&'static str> {
    match kind {
        "dashboard" => Some("dashboards"),
        "chart" => Some("charts"),
        "dataset" => Some("datasets"),
        "folder" => Some("folders"),
        "notebook" => Some("notebooks"),
        _ => None,
    }
}

/// The action a caller must hold on the object to manage its shares. Notebooks
/// have no `notebook:manage` action, so notebook sharing requires `notebook:write`
/// (Editor); every BI kind uses `<kind>:manage`.
fn manage_action(kind: &str) -> String {
    if kind == "notebook" {
        "notebook:write".to_string()
    } else {
        format!("{kind}:manage")
    }
}

/// Resolve `(kind, id)` to the `ObjectRef` (folder + workspace ancestry) the PDP
/// needs. `NotFound` if the object is absent, `InvalidInput` for an unknown kind.
pub async fn object_ref(db: &Db, kind: &str, id: Uuid) -> Result<ObjectRef, Error> {
    let table = object_table(kind).ok_or_else(|| Error::InvalidInput(format!("kind: {kind}")))?;
    let sql = if kind == "folder" {
        "SELECT NULL::uuid AS folder_id, workspace_id FROM analytics.folders WHERE id = $1"
            .to_string()
    } else {
        format!("SELECT folder_id, workspace_id FROM analytics.{table} WHERE id = $1")
    };
    let row: Option<(Option<Uuid>, Uuid)> = sqlx::query_as(&sql)
        .bind(id)
        .fetch_optional(&db.pool)
        .await
        .map_err(|e| Error::Internal(e.into()))?;
    let (folder_id, workspace_id) = row.ok_or_else(|| Error::NotFound(kind.to_string()))?;
    Ok(ObjectRef {
        kind: kind.to_string(),
        id,
        folder_id,
        workspace_id: egret_core::WorkspaceId(workspace_id),
    })
}

// ── DTOs ─────────────────────────────────────────────────────────────────────

/// Body of a share grant: give `principal` a workspace `role` on this object.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ObjectGrantBody {
    /// `user` | `group`.
    pub principal_type: String,
    pub principal_id: Uuid,
    /// `ws_viewer` (Viewer, read) | `ws_editor` (Editor, manage). For a "Blocked"
    /// grant, send `ws_editor` with `effect="deny"`.
    pub role: String,
    /// `allow` (default) | `deny`. A deny blocks the principal on this object.
    #[serde(default = "default_effect")]
    pub effect: String,
}

fn default_effect() -> String {
    "allow".to_string()
}

/// A single object-scoped grant on the object.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ObjectGrantDto {
    pub id: Uuid,
    pub principal_type: String,
    pub principal_id: Uuid,
    /// Human-readable grantee name: a user's email or a group's name. `None` if
    /// the principal row could not be resolved (e.g. deleted).
    pub principal_name: Option<String>,
    /// The workspace role name (`ws_viewer` | `ws_editor`).
    pub role: String,
    /// `allow` | `deny`.
    pub effect: String,
}

pub fn shares_router() -> Router<BiState> {
    Router::new()
        .route(
            "/ws/{workspace}/objects/{kind}/{id}/grants",
            put(grant_put).get(grant_list),
        )
        .route(
            "/ws/{workspace}/objects/{kind}/{id}/grants/{grant_id}",
            delete(grant_delete),
        )
}

/// Validate the share role name and reject anything outside the sharable set.
fn valid_role(role: &str) -> Result<(), BiApiError> {
    match role {
        "ws_viewer" | "ws_editor" => Ok(()),
        _ => Err(Error::InvalidInput(format!("role: {role}")).into()),
    }
}

/// Validate the principal type.
fn valid_principal_type(pt: &str) -> Result<(), BiApiError> {
    match pt {
        "user" | "group" => Ok(()),
        _ => Err(Error::InvalidInput(format!("principal_type: {pt}")).into()),
    }
}

/// Validate the grant effect.
fn valid_effect(e: &str) -> Result<(), BiApiError> {
    match e {
        "allow" | "deny" => Ok(()),
        _ => Err(Error::InvalidInput(format!("effect: {e}")).into()),
    }
}

/// Resolve `(kind, id)` to its `ObjectRef` and assert it lives in the path
/// workspace (else cross-tenant → `NotFound`). Shared by the share API and the
/// per-object BI handlers so both apply the same object + workspace guard before
/// `require_object`.
pub(crate) async fn resolve(
    st: &BiState,
    ctx: &WorkspaceContext,
    kind: &str,
    id: Uuid,
) -> Result<ObjectRef, BiApiError> {
    let obj = object_ref(&st.db, kind, id)
        .await
        .map_err(BiApiError::from)?;
    // The object must live in the workspace named on the path (else cross-tenant).
    if obj.workspace_id != ctx.workspace_id {
        return Err(Error::NotFound(kind.to_string()).into());
    }
    Ok(obj)
}

#[utoipa::path(
    put,
    path = "/api/ws/{workspace}/objects/{kind}/{id}/grants",
    tag = "shares",
    params(
        WorkspacePath,
        ("kind" = String, Path, description = "Object kind: dashboard | chart | dataset | folder"),
        ("id" = Uuid, Path, description = "Object id"),
    ),
    request_body = ObjectGrantBody,
    responses(
        (status = 200, description = "Grant created (or already existed)", body = ObjectGrantDto),
        (status = 400, description = "Unknown kind, role, or principal_type", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing <kind>:manage on the object", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such object in this workspace", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn grant_put(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, kind, id)): Path<(String, String, Uuid)>,
    Json(b): Json<ObjectGrantBody>,
) -> Result<Json<ObjectGrantDto>, BiApiError> {
    let obj = resolve(&st, &ctx, &kind, id).await?;
    require_object(&*st.pdp, &ctx, &manage_action(&kind), obj)
        .await
        .map_err(BiApiError::from)?;
    valid_principal_type(&b.principal_type)?;
    valid_role(&b.role)?;
    valid_effect(&b.effect)?;

    let role_id: Uuid =
        sqlx::query_scalar("SELECT id FROM roles WHERE scope = 'workspace' AND name = $1")
            .bind(&b.role)
            .fetch_optional(&st.db.pool)
            .await
            .map_err(|e| BiApiError::from(crate::BiError::Db(e)))?
            .ok_or_else(|| BiApiError::from(Error::InvalidInput(format!("role: {}", b.role))))?;

    // ON CONFLICT DO NOTHING (unique on principal+role+scope+scope_id); re-read to
    // return the canonical row whether we inserted or it already existed.
    sqlx::query(
        "INSERT INTO access_grants \
           (principal_type, principal_id, role_id, scope_type, object_type, scope_id, effect, granted_by) \
         VALUES ($1, $2, $3, 'object', $4, $5, $6, $7) ON CONFLICT DO NOTHING",
    )
    .bind(&b.principal_type)
    .bind(b.principal_id)
    .bind(role_id)
    .bind(&kind)
    .bind(id)
    .bind(&b.effect)
    .bind(ctx.principal.user_id.0)
    .execute(&st.db.pool)
    .await
    .map_err(|e| BiApiError::from(crate::BiError::Db(e)))?;

    let grant_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM access_grants \
         WHERE principal_type = $1 AND principal_id = $2 AND role_id = $3 \
           AND scope_type = 'object' AND scope_id = $4 AND effect = $5",
    )
    .bind(&b.principal_type)
    .bind(b.principal_id)
    .bind(role_id)
    .bind(id)
    .bind(&b.effect)
    .fetch_one(&st.db.pool)
    .await
    .map_err(|e| BiApiError::from(crate::BiError::Db(e)))?;

    st.audit.emit(egret_audit::AuditEvent::Mutation {
        actor: Some(ctx.principal.user_id.0),
        workspace_id: Some(ctx.workspace_id.0),
        action: "object_grant:put".into(),
        resource_type: "object".into(),
        resource_id: Some(format!("{kind}:{id}")),
        detail: serde_json::json!({
            "principal_type": b.principal_type,
            "principal_id": b.principal_id,
            "role": b.role,
            "effect": b.effect,
        }),
    });

    Ok(Json(ObjectGrantDto {
        id: grant_id,
        principal_type: b.principal_type,
        principal_id: b.principal_id,
        // The FE re-fetches the list (which resolves names) after a PUT, so we
        // don't resolve here.
        principal_name: None,
        role: b.role,
        effect: b.effect,
    }))
}

#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/objects/{kind}/{id}/grants/{grant_id}",
    tag = "shares",
    params(
        WorkspacePath,
        ("kind" = String, Path, description = "Object kind: dashboard | chart | dataset | folder"),
        ("id" = Uuid, Path, description = "Object id"),
        ("grant_id" = Uuid, Path, description = "Grant id to revoke"),
    ),
    responses(
        (status = 200, description = "Grant revoked (idempotent)", body = egret_core::Ack),
        (status = 400, description = "Unknown kind", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing <kind>:manage on the object", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such object in this workspace", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn grant_delete(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, kind, id, grant_id)): Path<(String, String, Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    let obj = resolve(&st, &ctx, &kind, id).await?;
    require_object(&*st.pdp, &ctx, &manage_action(&kind), obj)
        .await
        .map_err(BiApiError::from)?;
    // Scope the delete by the object so a manage holder can't revoke an unrelated grant.
    sqlx::query(
        "DELETE FROM access_grants \
         WHERE id = $1 AND scope_type = 'object' AND scope_id = $2",
    )
    .bind(grant_id)
    .bind(id)
    .execute(&st.db.pool)
    .await
    .map_err(|e| BiApiError::from(crate::BiError::Db(e)))?;
    st.audit.emit(egret_audit::AuditEvent::Mutation {
        actor: Some(ctx.principal.user_id.0),
        workspace_id: Some(ctx.workspace_id.0),
        action: "object_grant:delete".into(),
        resource_type: "object".into(),
        resource_id: Some(format!("{kind}:{id}")),
        detail: serde_json::json!({ "grant_id": grant_id }),
    });
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/objects/{kind}/{id}/grants",
    tag = "shares",
    params(
        WorkspacePath,
        ("kind" = String, Path, description = "Object kind: dashboard | chart | dataset | folder"),
        ("id" = Uuid, Path, description = "Object id"),
    ),
    responses(
        (status = 200, description = "Direct object grants", body = Vec<ObjectGrantDto>),
        (status = 400, description = "Unknown kind", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing <kind>:manage on the object", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such object in this workspace", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn grant_list(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, kind, id)): Path<(String, String, Uuid)>,
) -> Result<Json<Vec<ObjectGrantDto>>, BiApiError> {
    let obj = resolve(&st, &ctx, &kind, id).await?;
    // Listing the ACL exposes every grantee, so it is manage-only (matching
    // grant_put/grant_delete) — a merely read-shared recipient must not be able
    // to enumerate who else the object is shared with.
    require_object(&*st.pdp, &ctx, &manage_action(&kind), obj)
        .await
        .map_err(BiApiError::from)?;
    // LEFT JOIN resolves the grantee's display name: a user's email or a group's
    // name (following egret-auth `assignees_named`). `email` is `citext`, so cast
    // to `text` for a uniform `Option<String>` column.
    let rows: Vec<(Uuid, String, Uuid, Option<String>, String, String)> = sqlx::query_as(
        "SELECT ag.id, ag.principal_type, ag.principal_id, \
                COALESCE(u.email::text, g.name) AS principal_name, \
                r.name AS role, ag.effect \
         FROM access_grants ag JOIN roles r ON r.id = ag.role_id \
         LEFT JOIN users u ON ag.principal_type = 'user' AND u.id = ag.principal_id \
         LEFT JOIN groups g ON ag.principal_type = 'group' AND g.id = ag.principal_id \
         WHERE ag.scope_type = 'object' AND ag.scope_id = $1 \
         ORDER BY ag.created_at",
    )
    .bind(id)
    .fetch_all(&st.db.pool)
    .await
    .map_err(|e| BiApiError::from(crate::BiError::Db(e)))?;
    Ok(Json(
        rows.into_iter()
            .map(|(gid, pt, pid, name, role, effect)| ObjectGrantDto {
                id: gid,
                principal_type: pt,
                principal_id: pid,
                principal_name: name,
                role,
                effect,
            })
            .collect(),
    ))
}

/// OpenAPI fragment for the object-share router.
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(grant_put, grant_delete, grant_list),
    components(schemas(ObjectGrantBody, ObjectGrantDto)),
    tags((name = "shares", description = "Per-object grants: share a dashboard / chart / dataset / folder with a user or group at a workspace role")),
)]
pub struct BiSharesApi;
