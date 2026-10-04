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
        "saved_query" => Some("saved_queries"),
        "app" => Some("apps"),
        _ => None,
    }
}

/// The action a caller must hold on the object to manage its shares (and its
/// tags), one literal arm per kind `object_table` accepts. Every kind uses
/// `<kind>:manage`.
///
/// Literals rather than `format!("{kind}:manage")` so `catalog_drift` can see
/// each key as enforced (this is one of its `PERM_TABLE_FNS`). `None` for a kind
/// `object_table` does not know.
fn manage_action(kind: &str) -> Option<&'static str> {
    match kind {
        "dashboard" => Some("dashboard:manage"),
        "chart" => Some("chart:manage"),
        "dataset" => Some("dataset:manage"),
        "folder" => Some("folder:manage"),
        "saved_query" => Some("saved_query:manage"),
        "app" => Some("app:manage"),
        _ => None,
    }
}

/// `manage_action` as the share/tag handlers need it. In practice an unknown kind
/// never reaches here — `resolve` (via `object_ref`) rejects it first as
/// `InvalidInput` — so the `NotFound` is only a backstop for a kind `object_table`
/// knows but `manage_action` does not.
fn manage_action_for(kind: &str) -> Result<&'static str, BiApiError> {
    manage_action(kind).ok_or_else(|| Error::NotFound("object kind".to_string()).into())
}

/// The action a caller must hold on the object to read it (and its tags). Every
/// kind uses `<kind>:read` (e.g. `app:read`).
fn read_action(kind: &str) -> String {
    format!("{kind}:read")
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
    let row: Option<(Option<Uuid>, Uuid)> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
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
    /// A user's email, a group's name, or a UUID.
    pub principal_id: String,
    /// `ws_viewer` (Viewer, read) | `ws_editor` (Editor, manage). For a "Blocked"
    /// grant, send `ws_editor` with `effect="deny"`.
    pub role: String,
    /// `allow` (default) | `deny`. A deny blocks the principal on this object.
    #[serde(default = "default_effect")]
    pub effect: String,
    /// Optional expiry (RFC3339). `None` = never expires (unchanged default).
    /// Enforced at read time by the PDP/admission predicates, not here.
    #[serde(default)]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
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
    /// Optional expiry (P2a). `None` = never expires. This is the object's own
    /// direct-grant list (manage-only), so an expired grant is still listed
    /// here — only the read-time admission predicates exclude expired grants.
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
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
        .route(
            "/ws/{workspace}/objects/{kind}/{id}/tags",
            axum::routing::get(object_tags_list),
        )
        .route(
            "/ws/{workspace}/objects/{kind}/{id}/tags/{tag_id}",
            put(object_tag_assign).delete(object_tag_unassign),
        )
}

/// A classification tag carried by an object (Access P3.1). Local snake_case DTO
/// so egret-bi does not depend on egret-controlplane. `assignment_count` is the
/// tag's global assignment count (as computed by `TagRepo`).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TagDto {
    pub id: Uuid,
    pub key: String,
    pub name: String,
    pub description: String,
    pub color: String,
    pub assignment_count: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<crate::tags::Tag> for TagDto {
    fn from(t: crate::tags::Tag) -> Self {
        TagDto {
            id: t.id,
            key: t.key,
            name: t.name,
            description: t.description,
            color: t.color,
            assignment_count: t.assignment_count,
            created_at: t.created_at,
        }
    }
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

/// Resolve a principal identifier to its id: a real UUID passes through; else a
/// user is looked up by email and a group by name. `NotFound` if unknown.
async fn resolve_principal_id(db: &Db, ptype: &str, ident: &str) -> Result<Uuid, Error> {
    if let Ok(u) = Uuid::parse_str(ident) {
        return Ok(u);
    }
    let sql = match ptype {
        "user" => "SELECT id FROM users WHERE email = $1",
        "group" => "SELECT id FROM groups WHERE name = $1",
        _ => return Err(Error::InvalidInput(format!("bad principal_type: {ptype}"))),
    };
    sqlx::query_scalar(sql)
        .bind(ident)
        .fetch_optional(&db.pool)
        .await
        .map_err(|e| Error::Internal(e.into()))?
        .ok_or_else(|| Error::NotFound(format!("{ptype}: {ident}")))
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
        ("kind" = String, Path, description = "Object kind: dashboard | chart | dataset | folder | notebook | saved_query | app"),
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
    require_object(&*st.pdp, &ctx, manage_action_for(&kind)?, obj)
        .await
        .map_err(BiApiError::from)?;
    valid_principal_type(&b.principal_type)?;
    valid_role(&b.role)?;
    valid_effect(&b.effect)?;

    let pid = resolve_principal_id(&st.db, &b.principal_type, &b.principal_id)
        .await
        .map_err(BiApiError::from)?;

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
           (principal_type, principal_id, role_id, scope_type, object_type, scope_id, effect, granted_by, expires_at) \
         VALUES ($1, $2, $3, 'object', $4, $5, $6, $7, $8) ON CONFLICT DO NOTHING",
    )
    .bind(&b.principal_type)
    .bind(pid)
    .bind(role_id)
    .bind(&kind)
    .bind(id)
    .bind(&b.effect)
    .bind(ctx.principal.user_id.0)
    .bind(b.expires_at)
    .execute(&st.db.pool)
    .await
    .map_err(|e| BiApiError::from(crate::BiError::Db(e)))?;

    // Re-read the canonical row (not `b.expires_at`): `ON CONFLICT DO NOTHING`
    // means a pre-existing grant keeps its stored expiry, which may differ
    // from what this PUT asked for.
    let (grant_id, expires_at): (Uuid, Option<chrono::DateTime<chrono::Utc>>) = sqlx::query_as(
        "SELECT id, expires_at FROM access_grants \
         WHERE principal_type = $1 AND principal_id = $2 AND role_id = $3 \
           AND scope_type = 'object' AND scope_id = $4 AND effect = $5",
    )
    .bind(&b.principal_type)
    .bind(pid)
    .bind(role_id)
    .bind(id)
    .bind(&b.effect)
    .fetch_one(&st.db.pool)
    .await
    .map_err(|e| BiApiError::from(crate::BiError::Db(e)))?;

    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "object_grant:put",
        "object",
        Some(format!("{kind}:{id}")),
        serde_json::json!({
            "principal_type": b.principal_type,
            "principal_id": b.principal_id,
            "role": b.role,
            "effect": b.effect,
        }),
    ));

    Ok(Json(ObjectGrantDto {
        id: grant_id,
        principal_type: b.principal_type,
        principal_id: pid,
        // The FE re-fetches the list (which resolves names) after a PUT, so we
        // don't resolve here.
        principal_name: None,
        role: b.role,
        effect: b.effect,
        expires_at,
    }))
}

#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/objects/{kind}/{id}/grants/{grant_id}",
    tag = "shares",
    params(
        WorkspacePath,
        ("kind" = String, Path, description = "Object kind: dashboard | chart | dataset | folder | notebook | saved_query | app"),
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
    require_object(&*st.pdp, &ctx, manage_action_for(&kind)?, obj)
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
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "object_grant:delete",
        "object",
        Some(format!("{kind}:{id}")),
        serde_json::json!({ "grant_id": grant_id }),
    ));
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/objects/{kind}/{id}/grants",
    tag = "shares",
    params(
        WorkspacePath,
        ("kind" = String, Path, description = "Object kind: dashboard | chart | dataset | folder | notebook | saved_query | app"),
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
    require_object(&*st.pdp, &ctx, manage_action_for(&kind)?, obj)
        .await
        .map_err(BiApiError::from)?;
    // LEFT JOIN resolves the grantee's display name: a user's email or a group's
    // name (following egret-auth `assignees_named`). `email` is `citext`, so cast
    // to `text` for a uniform `Option<String>` column.
    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        Uuid,
        String,
        Uuid,
        Option<String>,
        String,
        String,
        Option<chrono::DateTime<chrono::Utc>>,
    )> = sqlx::query_as(
        "SELECT ag.id, ag.principal_type, ag.principal_id, \
                COALESCE(u.email::text, g.name) AS principal_name, \
                r.name AS role, ag.effect, ag.expires_at \
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
            .map(
                |(gid, pt, pid, name, role, effect, expires_at)| ObjectGrantDto {
                    id: gid,
                    principal_type: pt,
                    principal_id: pid,
                    principal_name: name,
                    role,
                    effect,
                    expires_at,
                },
            )
            .collect(),
    ))
}

// ── Per-object classification tags (Access P3.1) ─────────────────────────────
// Reads gate on object READ (`require_object(read_action(kind))`), writes on
// object MANAGE (`require_object(manage_action(kind))`), same as the per-object
// BI handlers. Cross-workspace targets 404 via `resolve`. Metadata only — no
// authorization decision reads these assignments in this phase.

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/objects/{kind}/{id}/tags",
    tag = "shares",
    params(
        WorkspacePath,
        ("kind" = String, Path, description = "Object kind: dashboard | chart | dataset | folder | notebook | saved_query | app"),
        ("id" = Uuid, Path, description = "Object id"),
    ),
    responses(
        (status = 200, description = "The object's classification tags", body = Vec<TagDto>),
        (status = 400, description = "Unknown kind", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing <kind>:read on the object", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such object in this workspace", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn object_tags_list(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, kind, id)): Path<(String, String, Uuid)>,
) -> Result<Json<Vec<TagDto>>, BiApiError> {
    let obj = resolve(&st, &ctx, &kind, id).await?;
    require_object(&*st.pdp, &ctx, &read_action(&kind), obj)
        .await
        .map_err(BiApiError::from)?;
    let tags = crate::tags::TagRepo::new(st.db.clone())
        .tags_for_object(&kind, id)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(tags.into_iter().map(TagDto::from).collect()))
}

#[utoipa::path(
    put,
    path = "/api/ws/{workspace}/objects/{kind}/{id}/tags/{tag_id}",
    tag = "shares",
    params(
        WorkspacePath,
        ("kind" = String, Path, description = "Object kind: dashboard | chart | dataset | folder | notebook | saved_query | app"),
        ("id" = Uuid, Path, description = "Object id"),
        ("tag_id" = Uuid, Path, description = "Tag id to assign"),
    ),
    responses(
        (status = 200, description = "Tag assigned (idempotent)", body = egret_core::Ack),
        (status = 400, description = "Unknown kind", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing <kind>:manage on the object", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such object in this workspace", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn object_tag_assign(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, kind, id, tag_id)): Path<(String, String, Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    let obj = resolve(&st, &ctx, &kind, id).await?;
    require_object(&*st.pdp, &ctx, manage_action_for(&kind)?, obj)
        .await
        .map_err(BiApiError::from)?;
    crate::tags::TagRepo::new(st.db.clone())
        .assign(tag_id, &kind, id, Some(ctx.principal.user_id.0))
        .await
        .map_err(BiApiError::from)?;
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "tag:assign",
        "object",
        Some(format!("{kind}:{id}")),
        serde_json::json!({ "tag_id": tag_id }),
    ));
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/objects/{kind}/{id}/tags/{tag_id}",
    tag = "shares",
    params(
        WorkspacePath,
        ("kind" = String, Path, description = "Object kind: dashboard | chart | dataset | folder | notebook | saved_query | app"),
        ("id" = Uuid, Path, description = "Object id"),
        ("tag_id" = Uuid, Path, description = "Tag id to unassign"),
    ),
    responses(
        (status = 200, description = "Tag unassigned (idempotent)", body = egret_core::Ack),
        (status = 400, description = "Unknown kind", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing <kind>:manage on the object", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such object in this workspace", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn object_tag_unassign(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, kind, id, tag_id)): Path<(String, String, Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    let obj = resolve(&st, &ctx, &kind, id).await?;
    require_object(&*st.pdp, &ctx, manage_action_for(&kind)?, obj)
        .await
        .map_err(BiApiError::from)?;
    crate::tags::TagRepo::new(st.db.clone())
        .unassign(tag_id, &kind, id)
        .await
        .map_err(BiApiError::from)?;
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "tag:unassign",
        "object",
        Some(format!("{kind}:{id}")),
        serde_json::json!({ "tag_id": tag_id }),
    ));
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// OpenAPI fragment for the object-share router.
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        grant_put,
        grant_delete,
        grant_list,
        object_tags_list,
        object_tag_assign,
        object_tag_unassign,
    ),
    components(schemas(ObjectGrantBody, ObjectGrantDto, TagDto)),
    tags((name = "shares", description = "Per-object grants: share a dashboard / chart / dataset / folder with a user or group at a workspace role")),
)]
pub struct BiSharesApi;

#[cfg(test)]
mod tests {
    use super::{manage_action, object_table};

    /// Pins the action each kind requires, plus `app` from Data Apps P2. Every
    /// kind `object_table` accepts has a manage action, and nothing else does.
    /// `notebook` is pinned among the rejected kinds: Data Apps P6 retired
    /// notebook sharing, so it must resolve the same as any unknown kind.
    #[test]
    fn manage_action_per_kind_is_unchanged() {
        let expected = [
            ("dashboard", "dashboard:manage"),
            ("chart", "chart:manage"),
            ("dataset", "dataset:manage"),
            ("folder", "folder:manage"),
            ("saved_query", "saved_query:manage"),
            ("app", "app:manage"),
        ];
        for (kind, action) in expected {
            assert!(object_table(kind).is_some(), "{kind} must be a share kind");
            assert_eq!(manage_action(kind), Some(action), "{kind}");
        }
        for kind in [
            "",
            "alert",
            "data_api_endpoint",
            "apps",
            "Dashboard",
            "notebook",
        ] {
            assert_eq!(object_table(kind), None, "{kind}");
            assert_eq!(manage_action(kind), None, "{kind}");
        }
    }
}
