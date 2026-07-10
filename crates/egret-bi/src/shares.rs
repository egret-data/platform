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
        _ => None,
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
pub struct GrantBody {
    /// `user` | `group`.
    pub principal_type: String,
    pub principal_id: Uuid,
    /// `ws_viewer` (Viewer, read) | `ws_editor` (Editor, manage).
    pub role: String,
}

/// A single object-scoped grant on the object.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct GrantDto {
    pub id: Uuid,
    pub principal_type: String,
    pub principal_id: Uuid,
    /// The workspace role name (`ws_viewer` | `ws_editor`).
    pub role: String,
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

async fn resolve(
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
    request_body = GrantBody,
    responses(
        (status = 200, description = "Grant created (or already existed)", body = GrantDto),
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
    Json(b): Json<GrantBody>,
) -> Result<Json<GrantDto>, BiApiError> {
    let obj = resolve(&st, &ctx, &kind, id).await?;
    require_object(&*st.pdp, &ctx, &format!("{kind}:manage"), obj)
        .await
        .map_err(BiApiError::from)?;
    valid_principal_type(&b.principal_type)?;
    valid_role(&b.role)?;

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
           (principal_type, principal_id, role_id, scope_type, object_type, scope_id, granted_by) \
         VALUES ($1, $2, $3, 'object', $4, $5, $6) ON CONFLICT DO NOTHING",
    )
    .bind(&b.principal_type)
    .bind(b.principal_id)
    .bind(role_id)
    .bind(&kind)
    .bind(id)
    .bind(ctx.principal.user_id.0)
    .execute(&st.db.pool)
    .await
    .map_err(|e| BiApiError::from(crate::BiError::Db(e)))?;

    let grant_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM access_grants \
         WHERE principal_type = $1 AND principal_id = $2 AND role_id = $3 \
           AND scope_type = 'object' AND scope_id = $4",
    )
    .bind(&b.principal_type)
    .bind(b.principal_id)
    .bind(role_id)
    .bind(id)
    .fetch_one(&st.db.pool)
    .await
    .map_err(|e| BiApiError::from(crate::BiError::Db(e)))?;

    Ok(Json(GrantDto {
        id: grant_id,
        principal_type: b.principal_type,
        principal_id: b.principal_id,
        role: b.role,
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
    require_object(&*st.pdp, &ctx, &format!("{kind}:manage"), obj)
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
        (status = 200, description = "Direct object grants", body = Vec<GrantDto>),
        (status = 400, description = "Unknown kind", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing <kind>:read on the object", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such object in this workspace", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn grant_list(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, kind, id)): Path<(String, String, Uuid)>,
) -> Result<Json<Vec<GrantDto>>, BiApiError> {
    let obj = resolve(&st, &ctx, &kind, id).await?;
    require_object(&*st.pdp, &ctx, &format!("{kind}:read"), obj)
        .await
        .map_err(BiApiError::from)?;
    let rows: Vec<(Uuid, String, Uuid, String)> = sqlx::query_as(
        "SELECT ag.id, ag.principal_type, ag.principal_id, r.name AS role \
         FROM access_grants ag JOIN roles r ON r.id = ag.role_id \
         WHERE ag.scope_type = 'object' AND ag.scope_id = $1 \
         ORDER BY ag.created_at",
    )
    .bind(id)
    .fetch_all(&st.db.pool)
    .await
    .map_err(|e| BiApiError::from(crate::BiError::Db(e)))?;
    Ok(Json(
        rows.into_iter()
            .map(|(gid, pt, pid, role)| GrantDto {
                id: gid,
                principal_type: pt,
                principal_id: pid,
                role,
            })
            .collect(),
    ))
}

/// OpenAPI fragment for the object-share router.
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(grant_put, grant_delete, grant_list),
    components(schemas(GrantBody, GrantDto)),
    tags((name = "shares", description = "Per-object grants: share a dashboard / chart / dataset / folder with a user or group at a workspace role")),
)]
pub struct BiSharesApi;
