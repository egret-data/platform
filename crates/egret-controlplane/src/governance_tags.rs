//! DG frontend redesign #1 (spec §2.2 "Object header", §2.3 "Not new"): `GET
//! /governance/tags/{catalog_id}?schema=&table=` — the classification tags on
//! one table and on its columns, for the Governance Data page. Read-only;
//! gated like browse on `policy:read` OR `tag:read`
//! ([`require_governance_read`]), so a steward who cannot read the catalog
//! registry can still see what an object is tagged with.
//!
//! **One store.** Tags are read with the same repository function the
//! catalog's own `GET /control-plane/warehouses/{id}/resource-tags` uses
//! ([`CatalogTagRepo::tags_for_table`]) and returned in its shape
//! ([`ResourceTagDto`]). That route keeps its own permission
//! (`catalog_registry:read`); this one is a second door onto the same rows,
//! not a widening of it.
//!
//! **Only egret catalogs carry tags.** Catalog tags are keyed by warehouse
//! (`catalog_tag_assignments.warehouse_id`), and only tables and columns can be
//! tagged (migration 0177: schema and table are never empty). A catalog that
//! is not egret's has no warehouse, so the answer says `supported: false`
//! with no list, never an empty list a page would read as "no tags".

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use egret_auth::principal::Auth;
use egret_bi::catalog_tags::ResourceTagDto;
use egret_bi::CatalogTagRepo;
use egret_core::http::ErrorBody;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::CpError;
use crate::governance_browse::{load_catalog, require_governance_read};
use crate::CpState;

pub fn router() -> Router<CpState> {
    Router::new().route("/governance/tags/{catalog_id}", get(object_tags))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ObjectTagsQuery {
    /// The table's schema (dotted for a nested egret namespace).
    pub schema: String,
    /// The table or view.
    pub table: String,
}

/// The tags on one table and on its columns.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObjectTags {
    /// `false` for a catalog that is not egret's: its objects cannot be
    /// tagged (tags are keyed by an egret warehouse), and `assignments` is
    /// `null`.
    pub supported: bool,
    /// Each tag directly on the table (`column` empty) or on one of its
    /// columns, sorted by column then tag key. `null` exactly when not
    /// `supported`.
    pub assignments: Option<Vec<ResourceTagDto>>,
}

#[utoipa::path(
    get,
    path = "/api/control-plane/governance/tags/{catalog_id}",
    tag = "control-plane-governance",
    params(
        ("catalog_id" = Uuid, Path, description = "Registry catalog id"),
        ObjectTagsQuery,
    ),
    responses(
        (status = 200, description = "The table's and its columns' tags, or that the catalog cannot carry tags", body = ObjectTags),
        (status = 400, description = "A missing or empty `schema` or `table`", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:read and tag:read permission", body = ErrorBody),
        (status = 404, description = "No such catalog", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn object_tags(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(catalog_id): Path<Uuid>,
    Query(q): Query<ObjectTagsQuery>,
) -> Result<Json<ObjectTags>, CpError> {
    require_governance_read(&st, &p).await?;
    if q.schema.is_empty() || q.table.is_empty() {
        return Err(CpError::BadRequest(
            "`schema` and `table` must not be empty".to_string(),
        ));
    }
    let catalog = load_catalog(&st, catalog_id)
        .await?
        .ok_or(CpError::NotFound)?;
    let Some(warehouse_id) = catalog.warehouse_id else {
        return Ok(Json(ObjectTags {
            supported: false,
            assignments: None,
        }));
    };
    let rows = CatalogTagRepo::new(st.db.clone())
        .tags_for_table(warehouse_id, &q.schema, &q.table)
        .await
        .map_err(|e| {
            tracing::error!(error = ?e, "governance: read object tags");
            CpError::Internal
        })?;
    Ok(Json(ObjectTags {
        supported: true,
        assignments: Some(rows.into_iter().map(ResourceTagDto::from).collect()),
    }))
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(object_tags),
    components(schemas(ObjectTags, ResourceTagDto, ErrorBody)),
    tags((name = "control-plane-governance", description = "Governance Data: browse a catalog and read an object's lineage (DG frontend redesign #1)")),
)]
pub struct ControlPlaneGovernanceTagsApi;
