//! Control-plane Tag catalog router (Access P3.1): the platform-wide
//! classification-tag catalog (create/read/update/delete), nested under
//! `/api/control-plane`.
//!
//! METADATA ONLY: no authorization decision reads these tags in this phase
//! (see `egret_bi::tags` for the repo-level note). `tag:read` gates the list;
//! every write (create/update/delete) gates on `tag:manage`. Neither is an
//! admin-graph action (they don't grant roles/permissions), so — unlike the
//! `access` module's role/permission writes — both are ordinary delegatable
//! permissions, gated the same way `secrets.rs`/`storage.rs` gate their reads
//! and writes: `egret_core::authz::require(&*st.pdp, &p, "...")` on the bare
//! `Auth` principal (these endpoints are not workspace-scoped).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, patch};
use axum::{Json, Router};
use egret_auth::principal::Auth;
use egret_bi::Tag;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use egret_core::http::ErrorBody;

use crate::error::CpError;
use crate::CpState;

pub fn router() -> Router<CpState> {
    Router::new()
        .route("/tags", get(list_tags).post(create_tag))
        .route("/tags/{id}", patch(update_tag).delete(delete_tag))
        .route(
            "/tags/{id}/access-policy",
            axum::routing::put(set_access_policy).delete(clear_access_policy),
        )
}

// snake_case (house convention here): matches the FE `Tag` type and the sibling
// object-tag `TagDto` in egret-bi, which the FE consumes with a single interface.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TagDto {
    pub id: String,
    pub key: String,
    pub name: String,
    pub description: String,
    pub color: String,
    pub assignment_count: i64,
    pub created_at: String,
    /// Classification access policy (Access P3.3): clearance role names a
    /// principal needs `>= 1` of to access objects carrying this tag. Empty = no
    /// gate (informational tag). Populated by the tag list from `list_policies`;
    /// empty on the create/update responses (those don't touch the policy).
    #[serde(default)]
    pub required_roles: Vec<String>,
}

impl From<Tag> for TagDto {
    fn from(t: Tag) -> Self {
        TagDto {
            id: t.id.to_string(),
            key: t.key,
            name: t.name,
            description: t.description,
            color: t.color,
            assignment_count: t.assignment_count,
            created_at: t.created_at.to_rfc3339(),
            required_roles: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
struct CreateTag {
    key: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    color: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
struct UpdateTag {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    color: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/control-plane/tags",
    tag = "control-plane-tags",
    responses(
        (status = 200, description = "The platform tag catalog, each with its live assignment count", body = Vec<TagDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing tag:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_tags(State(st): State<CpState>, Auth(p): Auth) -> Result<Json<Vec<TagDto>>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "tag:read").await?;
    let tags = st.tags.list().await?;
    // Overlay each tag's classification access policy (Access P3.3) so the FE can
    // show which tags are gated. One extra query, joined in memory by tag id.
    let policies: std::collections::HashMap<Uuid, Vec<String>> = st
        .classification_policies
        .list_policies()
        .await?
        .into_iter()
        .collect();
    Ok(Json(
        tags.into_iter()
            .map(|t| {
                let required_roles = policies.get(&t.id).cloned().unwrap_or_default();
                let mut dto = TagDto::from(t);
                dto.required_roles = required_roles;
                dto
            })
            .collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/tags",
    tag = "control-plane-tags",
    request_body = CreateTag,
    responses(
        (status = 201, description = "Tag created", body = TagDto),
        (status = 400, description = "Invalid tag key", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing tag:manage permission", body = ErrorBody),
        (status = 409, description = "A tag with that key already exists", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn create_tag(
    State(st): State<CpState>,
    Auth(p): Auth,
    Json(body): Json<CreateTag>,
) -> Result<(StatusCode, Json<TagDto>), CpError> {
    egret_core::authz::require(&*st.pdp, &p, "tag:manage").await?;
    let tag = st
        .tags
        .create(
            &body.key,
            &body.name,
            body.description.as_deref().unwrap_or(""),
            body.color.as_deref().unwrap_or(""),
            Some(p.user_id.0),
        )
        .await?;
    Ok((StatusCode::CREATED, Json(TagDto::from(tag))))
}

#[utoipa::path(
    patch,
    path = "/api/control-plane/tags/{id}",
    tag = "control-plane-tags",
    params(("id" = Uuid, Path, description = "Tag id")),
    request_body = UpdateTag,
    responses(
        (status = 200, description = "Tag updated", body = TagDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing tag:manage permission", body = ErrorBody),
        (status = 404, description = "No such tag", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn update_tag(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateTag>,
) -> Result<Json<TagDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "tag:manage").await?;
    let tag = st
        .tags
        .update(
            id,
            body.name.as_deref(),
            body.description.as_deref(),
            body.color.as_deref(),
        )
        .await?
        .ok_or(CpError::NotFound)?;
    Ok(Json(TagDto::from(tag)))
}

#[utoipa::path(
    delete,
    path = "/api/control-plane/tags/{id}",
    tag = "control-plane-tags",
    params(("id" = Uuid, Path, description = "Tag id")),
    responses(
        (status = 204, description = "Tag deleted"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing tag:manage permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn delete_tag(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "tag:manage").await?;
    st.tags.delete(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
struct SetAccessPolicy {
    /// Clearance role NAMES; a principal needs `>= 1` to access objects carrying
    /// this tag. Empty = no gate (informational tag only).
    #[serde(default)]
    required_roles: Vec<String>,
}

#[utoipa::path(
    put,
    path = "/api/control-plane/tags/{id}/access-policy",
    tag = "control-plane-tags",
    params(("id" = Uuid, Path, description = "Tag id")),
    request_body = SetAccessPolicy,
    responses(
        (status = 200, description = "Classification access policy set"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing tag:manage permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn set_access_policy(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<SetAccessPolicy>,
) -> Result<StatusCode, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "tag:manage").await?;
    // The repo publishes `RolesChanged` on the shared bus so the Cedar PDP
    // reloads its policy set (the SAME reload event the role/permission CRUD
    // emits) — a future PDP loader reading `classification_policies` stays fresh.
    st.classification_policies
        .set_policy(id, &body.required_roles, Some(p.user_id.0))
        .await?;
    Ok(StatusCode::OK)
}

#[utoipa::path(
    delete,
    path = "/api/control-plane/tags/{id}/access-policy",
    tag = "control-plane-tags",
    params(("id" = Uuid, Path, description = "Tag id")),
    responses(
        (status = 200, description = "Classification access policy cleared"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing tag:manage permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn clear_access_policy(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "tag:manage").await?;
    st.classification_policies.clear_policy(id).await?;
    Ok(StatusCode::OK)
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(list_tags, create_tag, update_tag, delete_tag, set_access_policy, clear_access_policy),
    components(schemas(TagDto, CreateTag, UpdateTag, SetAccessPolicy, ErrorBody)),
    tags((name = "control-plane-tags", description = "Platform classification-tag catalog (Access P3.1) + access policies (P3.3)")),
)]
pub struct ControlPlaneTagsApi;
