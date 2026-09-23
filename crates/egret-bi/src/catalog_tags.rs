//! Catalog-resource tag targets (Access P3.2) — attach a classification `tag`
//! (from P3.1's `tags` catalog) to a CATALOG table/column. Unlike a BI object, a
//! catalog resource is NOT uuid-keyed: it is identified by
//! warehouse + schema + table + column, where an empty column (`""`) means a
//! table-level tag. The repo methods run parameterized SQL against migration
//! 0081's `catalog_tag_assignments` table; the router exposes control-plane
//! endpoints gated on the catalog-manage permission (`warehouse:update` for
//! writes, `warehouse:read` for reads), mirroring the warehouse_admin browse API.
//!
//! METADATA ONLY: no authorization decision / Ranger download reads these
//! assignments in this phase (tag-based masking policies are Task 2).

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use egret_auth::principal::Auth;
use egret_metastore::Db;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::BiApiError;
use crate::routes::BiState;
use crate::shares::TagDto;
use crate::tags::Tag;

/// The catalog-manage permission gating catalog-tag WRITES (assign/unassign);
/// the same `warehouse:update` used by the warehouse_admin mutating browse ops.
const WAREHOUSE_WRITE: &str = "warehouse:update";
/// The permission gating catalog-tag READS (list); the warehouse_admin read perm.
const WAREHOUSE_READ: &str = "warehouse:read";

/// A classification tag attached to a specific catalog resource. `column` is
/// empty (`""`) for a table-level assignment. `tag` is the full P3.1 tag row (its
/// `assignment_count` is the tag's global BI-object count, as `TagRepo` computes).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CatalogTagAssignment {
    pub schema: String,
    pub table: String,
    pub column: String,
    #[sqlx(flatten)]
    pub tag: Tag,
}

#[derive(Clone)]
pub struct CatalogTagRepo {
    db: Db,
}

impl CatalogTagRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Attach `tag_id` to a catalog resource. Idempotent — a re-assign is a no-op
    /// (ON CONFLICT DO NOTHING on the composite primary key). `column = ""` tags
    /// the whole table.
    ///
    /// Bumps `tag_version` on every policy service bound to this warehouse (see
    /// [`Self::bump_bound_tag_versions`]) in the SAME transaction, so the Ranger
    /// tag-download plugin re-polls `download_tags` instead of 304-ing forever.
    pub async fn assign_catalog(
        &self,
        tag_id: Uuid,
        warehouse_id: Uuid,
        schema: &str,
        table: &str,
        column: &str,
        assigned_by: Option<Uuid>,
    ) -> Result<(), BiApiError> {
        let mut tx = self
            .db
            .pool
            .begin()
            .await
            .map_err(crate::error::BiError::from)?;
        sqlx::query(
            "INSERT INTO catalog_tag_assignments \
                 (tag_id, warehouse_id, schema_name, table_name, column_name, assigned_by) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (tag_id, warehouse_id, schema_name, table_name, column_name) DO NOTHING",
        )
        .bind(tag_id)
        .bind(warehouse_id)
        .bind(schema)
        .bind(table)
        .bind(column)
        .bind(assigned_by)
        .execute(&mut *tx)
        .await
        .map_err(crate::error::BiError::from)?;
        Self::bump_bound_tag_versions(&mut tx, warehouse_id).await?;
        tx.commit().await.map_err(crate::error::BiError::from)?;
        Ok(())
    }

    /// Detach `tag_id` from a catalog resource (idempotent — deletes 0 or 1 row).
    /// Bumps the bound services' `tag_version` in the same transaction (see
    /// [`Self::assign_catalog`]).
    pub async fn unassign_catalog(
        &self,
        tag_id: Uuid,
        warehouse_id: Uuid,
        schema: &str,
        table: &str,
        column: &str,
    ) -> Result<(), BiApiError> {
        let mut tx = self
            .db
            .pool
            .begin()
            .await
            .map_err(crate::error::BiError::from)?;
        sqlx::query(
            "DELETE FROM catalog_tag_assignments \
             WHERE tag_id = $1 AND warehouse_id = $2 AND schema_name = $3 \
                 AND table_name = $4 AND column_name = $5",
        )
        .bind(tag_id)
        .bind(warehouse_id)
        .bind(schema)
        .bind(table)
        .bind(column)
        .execute(&mut *tx)
        .await
        .map_err(crate::error::BiError::from)?;
        Self::bump_bound_tag_versions(&mut tx, warehouse_id).await?;
        tx.commit().await.map_err(crate::error::BiError::from)?;
        Ok(())
    }

    /// Bump the `tag_version` of every policy service that serves
    /// `warehouse_id`'s catalog tags, so the Ranger tag plugin stops 304ing and
    /// re-downloads. Runs on the caller's transaction (cross-seam, same DB).
    ///
    /// The resolution itself — the registry∪legacy `profiles` union and the
    /// `tagService` fan-out the plugin actually polls — lives in
    /// `egret_catalog_registry::bump_bound_tag_versions` (SP3
    /// catalog-registry review C1/I1/N1/F-1). One implementation, two
    /// callers (assign and unassign), no drift: read that function for why
    /// each branch is there.
    async fn bump_bound_tag_versions(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        warehouse_id: Uuid,
    ) -> Result<(), BiApiError> {
        egret_catalog_registry::bump_bound_tag_versions(&mut **tx, warehouse_id)
            .await
            .map_err(crate::error::BiError::from)?;
        Ok(())
    }

    /// The tags carried by one EXACT resource (`column = ""` ⇒ the table itself),
    /// ordered by key. Distinguishes a table-level tag from a column-level tag.
    pub async fn tags_for_resource(
        &self,
        warehouse_id: Uuid,
        schema: &str,
        table: &str,
        column: &str,
    ) -> Result<Vec<Tag>, BiApiError> {
        let sql = "SELECT t.id, t.key, t.name, t.description, t.color, \
                 COUNT(ta.tag_id) AS assignment_count, t.created_at \
             FROM tags t \
             JOIN catalog_tag_assignments cta ON cta.tag_id = t.id \
                 AND cta.warehouse_id = $1 AND cta.schema_name = $2 \
                 AND cta.table_name = $3 AND cta.column_name = $4 \
             LEFT JOIN tag_assignments ta ON ta.tag_id = t.id \
             GROUP BY t.id ORDER BY t.key";
        Ok(sqlx::query_as::<_, Tag>(sql)
            .bind(warehouse_id)
            .bind(schema)
            .bind(table)
            .bind(column)
            .fetch_all(&self.db.pool)
            .await
            .map_err(crate::error::BiError::from)?)
    }

    /// Every catalog-tag assignment on a single table — the table-level tags AND
    /// all of its columns' tags — as `{schema, table, column, tag}` rows.
    pub async fn tags_for_table(
        &self,
        warehouse_id: Uuid,
        schema: &str,
        table: &str,
    ) -> Result<Vec<CatalogTagAssignment>, BiApiError> {
        let sql =
            list_select("cta.warehouse_id = $1 AND cta.schema_name = $2 AND cta.table_name = $3");
        Ok(
            sqlx::query_as::<_, CatalogTagAssignment>(sqlx::AssertSqlSafe(sql))
                .bind(warehouse_id)
                .bind(schema)
                .bind(table)
                .fetch_all(&self.db.pool)
                .await
                .map_err(crate::error::BiError::from)?,
        )
    }

    /// Every catalog-tag assignment in a warehouse, as `{schema, table, column,
    /// tag}` rows (the Task-2 `download_tags` builder enumerates these).
    pub async fn list_for_warehouse(
        &self,
        warehouse_id: Uuid,
    ) -> Result<Vec<CatalogTagAssignment>, BiApiError> {
        let sql = list_select("cta.warehouse_id = $1");
        Ok(
            sqlx::query_as::<_, CatalogTagAssignment>(sqlx::AssertSqlSafe(sql))
                .bind(warehouse_id)
                .fetch_all(&self.db.pool)
                .await
                .map_err(crate::error::BiError::from)?,
        )
    }
}

/// Build the shared `SELECT … {schema, table, column, tag}` statement. `predicate`
/// is a trusted (never user-interpolated) WHERE clause over aliased `cta`/`t`. A
/// single global-count subquery keeps `tag.assignment_count` meaningful without an
/// N-row LEFT JOIN blow-up.
fn list_select(predicate: &str) -> String {
    format!(
        "SELECT cta.schema_name AS schema, cta.table_name AS \"table\", \
             cta.column_name AS \"column\", \
             t.id, t.key, t.name, t.description, t.color, \
             COALESCE(cnt.assignment_count, 0) AS assignment_count, t.created_at \
         FROM catalog_tag_assignments cta \
         JOIN tags t ON t.id = cta.tag_id \
         LEFT JOIN ( \
             SELECT tag_id, COUNT(*) AS assignment_count \
             FROM tag_assignments GROUP BY tag_id \
         ) cnt ON cnt.tag_id = t.id \
         WHERE {predicate} \
         ORDER BY cta.schema_name, cta.table_name, cta.column_name, t.key"
    )
}

// ── Endpoints ────────────────────────────────────────────────────────────────

/// A catalog resource tag on the wire (snake_case). `column` is empty for a
/// table-level tag. The tag itself is nested under `tag` (the full P3.1 tag
/// shape, matching the FE `Tag` type) so the browse UI can read `a.tag`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ResourceTagDto {
    pub schema: String,
    pub table: String,
    pub column: String,
    pub tag: TagDto,
}

impl From<CatalogTagAssignment> for ResourceTagDto {
    fn from(a: CatalogTagAssignment) -> Self {
        ResourceTagDto {
            schema: a.schema,
            table: a.table,
            column: a.column,
            tag: a.tag.into(),
        }
    }
}

/// Query params selecting the table whose resource-tags to list.
#[derive(Debug, Deserialize)]
pub struct ResourceQuery {
    pub schema: String,
    pub table: String,
}

/// Body of an assign/unassign: the resource (`column` optional ⇒ table-level) +
/// the tag id.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ResourceTagBody {
    pub schema: String,
    pub table: String,
    /// Empty / omitted ⇒ a table-level tag.
    #[serde(default)]
    pub column: Option<String>,
    pub tag_id: Uuid,
}

/// Control-plane router for catalog table/column tag targets. Nested under
/// `/api` by `egret-server`, giving `/api/control-plane/warehouses/{id}/resource-tags`.
pub fn resource_tags_router() -> Router<BiState> {
    Router::new().route(
        "/control-plane/warehouses/{warehouse_id}/resource-tags",
        get(list_resource_tags)
            .put(assign_resource_tag)
            .delete(unassign_resource_tag),
    )
}

#[utoipa::path(
    get,
    path = "/api/control-plane/warehouses/{warehouse_id}/resource-tags",
    tag = "control-plane-catalog",
    params(
        ("warehouse_id" = Uuid, Path, description = "Warehouse id"),
        ("schema" = String, Query, description = "Schema name"),
        ("table" = String, Query, description = "Table name"),
    ),
    responses(
        (status = 200, description = "The table's tags plus its columns' tags", body = Vec<ResourceTagDto>),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing warehouse:read permission", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn list_resource_tags(
    State(st): State<BiState>,
    Auth(p): Auth,
    Path(warehouse_id): Path<Uuid>,
    Query(q): Query<ResourceQuery>,
) -> Result<Json<Vec<ResourceTagDto>>, BiApiError> {
    egret_core::authz::require(&*st.pdp, &p, WAREHOUSE_READ).await?;
    let rows = CatalogTagRepo::new(st.db.clone())
        .tags_for_table(warehouse_id, &q.schema, &q.table)
        .await?;
    Ok(Json(rows.into_iter().map(ResourceTagDto::from).collect()))
}

#[utoipa::path(
    put,
    path = "/api/control-plane/warehouses/{warehouse_id}/resource-tags",
    tag = "control-plane-catalog",
    params(("warehouse_id" = Uuid, Path, description = "Warehouse id")),
    request_body = ResourceTagBody,
    responses(
        (status = 200, description = "Tag assigned to the catalog resource (idempotent)", body = egret_core::Ack),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing warehouse:update permission", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn assign_resource_tag(
    State(st): State<BiState>,
    Auth(p): Auth,
    Path(warehouse_id): Path<Uuid>,
    Json(b): Json<ResourceTagBody>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    egret_core::authz::require(&*st.pdp, &p, WAREHOUSE_WRITE).await?;
    let column = b.column.as_deref().unwrap_or("");
    CatalogTagRepo::new(st.db.clone())
        .assign_catalog(
            b.tag_id,
            warehouse_id,
            &b.schema,
            &b.table,
            column,
            Some(p.user_id.0),
        )
        .await?;
    st.audit.emit(egret_audit::mutation_for(
        &p,
        None,
        "tag:assign",
        "catalog",
        Some(format!(
            "{warehouse_id}:{}.{}.{}",
            b.schema, b.table, column
        )),
        serde_json::json!({ "tag_id": b.tag_id }),
    ));
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    delete,
    path = "/api/control-plane/warehouses/{warehouse_id}/resource-tags",
    tag = "control-plane-catalog",
    params(("warehouse_id" = Uuid, Path, description = "Warehouse id")),
    request_body = ResourceTagBody,
    responses(
        (status = 200, description = "Tag removed from the catalog resource (idempotent)", body = egret_core::Ack),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing warehouse:update permission", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn unassign_resource_tag(
    State(st): State<BiState>,
    Auth(p): Auth,
    Path(warehouse_id): Path<Uuid>,
    Json(b): Json<ResourceTagBody>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    egret_core::authz::require(&*st.pdp, &p, WAREHOUSE_WRITE).await?;
    let column = b.column.as_deref().unwrap_or("");
    CatalogTagRepo::new(st.db.clone())
        .unassign_catalog(b.tag_id, warehouse_id, &b.schema, &b.table, column)
        .await?;
    st.audit.emit(egret_audit::mutation_for(
        &p,
        None,
        "tag:unassign",
        "catalog",
        Some(format!(
            "{warehouse_id}:{}.{}.{}",
            b.schema, b.table, column
        )),
        serde_json::json!({ "tag_id": b.tag_id }),
    ));
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// OpenAPI fragment for the catalog resource-tag router (Access P3.2). Merged
/// into `egret_bi::openapi()`; reuses the `control-plane-catalog` tag.
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(list_resource_tags, assign_resource_tag, unassign_resource_tag),
    components(schemas(ResourceTagDto, ResourceTagBody))
)]
pub struct CatalogTagsApi;
