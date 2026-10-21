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
    /// Classification clearance by GROUP (Access P3.4): group ids (as strings) a
    /// principal needs to belong to `>= 1` of to access objects carrying this tag.
    /// Cleared = `>= 1` required role OR `>= 1` required group; both empty = no
    /// gate. Populated by the tag list from `list_policies`.
    #[serde(default)]
    pub required_groups: Vec<String>,
    /// DG #4b: tables of egret catalogs carrying this tag on the table itself
    /// (distinct `(warehouse, schema, table)` with a table-level row). Filled by
    /// the list; 0 on the create/update responses.
    #[serde(default)]
    pub catalog_tables: i64,
    /// DG #4b: columns of egret catalogs carrying this tag. Filled by the list;
    /// 0 on the create/update responses.
    #[serde(default)]
    pub catalog_columns: i64,
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
            required_groups: Vec::new(),
            catalog_tables: 0,
            catalog_columns: 0,
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
    let policies: std::collections::HashMap<Uuid, (Vec<String>, Vec<Uuid>)> = st
        .classification_policies
        .list_policies()
        .await?
        .into_iter()
        .map(|(id, roles, groups)| (id, (roles, groups)))
        .collect();
    // DG #4b: the Data counts, one grouped query joined the same way.
    let counts = st.tags.catalog_counts().await?;
    Ok(Json(
        tags.into_iter()
            .map(|t| {
                let t_id = t.id;
                let (required_roles, required_groups) =
                    policies.get(&t.id).cloned().unwrap_or_default();
                let mut dto = TagDto::from(t);
                dto.required_roles = required_roles;
                // Emit group ids as strings so the FE consumes a single type.
                dto.required_groups = required_groups.iter().map(|g| g.to_string()).collect();
                (dto.catalog_tables, dto.catalog_columns) =
                    counts.get(&t_id).copied().unwrap_or_default();
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
        (status = 400, description = "Invalid tag key: `code` is `tag_key_invalid`", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing tag:manage permission", body = ErrorBody),
        (status = 409, description = "`code` is `tag_key_taken` (the key exists) or `tag_name_taken` (another classification has that name, ignoring case); `existing` names the holder", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn create_tag(
    State(st): State<CpState>,
    Auth(p): Auth,
    Json(body): Json<CreateTag>,
) -> Result<(StatusCode, Json<TagDto>), CpError> {
    egret_core::authz::require(&*st.pdp, &p, "tag:manage").await?;
    if let Some(existing) = st.tags.name_taken(&body.name, None).await? {
        return Err(egret_bi::BiError::tag_name_taken(&body.name, &existing).into());
    }
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
        (status = 400, description = "The new name is masked by a tag masking policy while this tag is attached to a whole TABLE (which would make that table unreadable rather than masked)", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing tag:manage permission", body = ErrorBody),
        (status = 404, description = "No such tag", body = ErrorBody),
        (status = 409, description = "`code` is `tag_name_taken`: another classification has that name, ignoring case; `existing` names the holder", body = ErrorBody),
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
    if let Some(name) = body.name.as_deref() {
        if let Some(existing) = st.tags.name_taken(name, Some(id)).await? {
            return Err(egret_bi::BiError::tag_name_taken(name, &existing).into());
        }
    }
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
        (status = 409, description = "`code` is `tag_in_use`: `rules` lists the rules that still select this classification", body = ErrorBody),
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
    /// this tag. Cleared = `>= 1` role OR `>= 1` group; both empty = no gate.
    #[serde(default)]
    required_roles: Vec<String>,
    /// Clearance GROUP ids (as strings; Access P3.4). Each is parsed as a `Uuid`
    /// before hitting the repo — a non-uuid entry is rejected `400 BadRequest`
    /// (never spliced unvalidated into a Cedar policy string).
    #[serde(default)]
    required_groups: Vec<String>,
}

/// What a clearance write actually reached — **SP4c review I5.**
///
/// The endpoint used to answer a bare `{"ok": true}`, so a clearance authored
/// on a Trino profile with no usable `tagService`, or one naming only groups,
/// was reported as saved while the compiler had emitted nothing at all and the
/// UI showed it as active. `skipped` carries exactly what the perm mutation
/// responses already carry (`SkipReport`, same schema) and for the stated
/// reason: an author needs to know at authoring time, not from a query that
/// later returns rows it should not.
///
/// An EMPTY `skipped` is the normal answer and means every profile a clearance
/// can reach compiled it.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
struct AccessPolicyWritten {
    /// Always `true` — the write committed, or this response was never built.
    ok: bool,
    /// Every profile/item the accompanying recompile could not express.
    skipped: Vec<egret_perm_compile::routes::SkipReport>,
}

impl AccessPolicyWritten {
    fn from_skips(skipped: Vec<egret_perm_compile::SkippedItem>) -> Self {
        AccessPolicyWritten {
            ok: true,
            skipped: skipped
                .into_iter()
                .map(egret_perm_compile::routes::SkipReport::from)
                .collect(),
        }
    }
}

#[utoipa::path(
    put,
    path = "/api/control-plane/tags/{id}/access-policy",
    tag = "control-plane-tags",
    params(("id" = Uuid, Path, description = "Tag id")),
    request_body = SetAccessPolicy,
    responses(
        (status = 200, description = "Classification access policy set", body = AccessPolicyWritten),
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
) -> Result<Json<AccessPolicyWritten>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "tag:manage").await?;
    // SECURITY (Access P3.4): parse each group id as a `Uuid` BEFORE it reaches
    // the repo / Cedar policy string. A non-uuid entry is rejected `400` — never
    // splice unvalidated text into a Cedar policy (the P3.3 injection lesson).
    let required_groups = body
        .required_groups
        .iter()
        .map(|g| {
            Uuid::parse_str(g)
                .map_err(|_| CpError::BadRequest(format!("required_groups must be uuids: {g:?}")))
        })
        .collect::<Result<Vec<Uuid>, _>>()?;
    // The repo publishes `RolesChanged` on the shared bus so the Cedar PDP
    // reloads its policy set (the SAME reload event the role/permission CRUD
    // emits) — the PDP loader reading `classification_policies` stays fresh.
    let skipped = st
        .classification_policies
        .set_policy(id, &body.required_roles, &required_groups, &p)
        .await?;
    // Return a JSON body (not a bare 200) so the FE `apiJson` can parse it —
    // an empty 200 body throws "Unexpected end of JSON input" client-side — and
    // carry what the recompile could NOT express (I5), rather than reporting a
    // clearance as saved when no engine was given one.
    Ok(Json(AccessPolicyWritten::from_skips(skipped)))
}

#[utoipa::path(
    delete,
    path = "/api/control-plane/tags/{id}/access-policy",
    tag = "control-plane-tags",
    params(("id" = Uuid, Path, description = "Tag id")),
    responses(
        (status = 200, description = "Classification access policy cleared", body = AccessPolicyWritten),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing tag:manage permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn clear_access_policy(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<AccessPolicyWritten>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "tag:manage").await?;
    let skipped = st.classification_policies.clear_policy(id, &p).await?;
    Ok(Json(AccessPolicyWritten::from_skips(skipped)))
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(list_tags, create_tag, update_tag, delete_tag, set_access_policy, clear_access_policy),
    components(schemas(
        TagDto,
        CreateTag,
        UpdateTag,
        SetAccessPolicy,
        AccessPolicyWritten,
        egret_perm_compile::routes::SkipReport,
        ErrorBody
    )),
    tags((name = "control-plane-tags", description = "Platform classification-tag catalog (Access P3.1) + access policies (P3.3)")),
)]
pub struct ControlPlaneTagsApi;
