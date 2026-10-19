//! DG #4b route C (spec `docs/en/specs/2026-10-19-dg-classification-design.md`
//! §4.4, Q3, Q5, CL11, CL12): `GET /classifications/{id}/usage`, where a
//! classification is used — the tables and columns of egret catalogs carrying
//! it, and the BI objects carrying it that the caller can read.
//!
//! Each BI object is checked with the caller's own read action through the PDP
//! (`egret_bi::shares` decides the action and the object's ancestry), so the
//! page never names an object its reader could not open. An unreadable object
//! is COUNTED only — no name, no workspace, no workspace count (Q5).
use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use egret_auth::principal::Auth;
use egret_core::authz::{AuthTarget, Resource};
use egret_core::http::ErrorBody;
use serde::Serialize;
use uuid::Uuid;

use crate::governance_browse::require_governance_read;
use crate::{CpError, CpState};

/// How many BI objects one request checks (CL12). Task 1 M4 measured 500
/// sequential checks (`object_ref` + `Pdp::authorize`) at 3.0–3.3 s in a
/// release build against Postgres in Docker Desktop, round-trip bound
/// (`docs/en/plans/2026-10-19-dg-classification-live.md` §1.3); the rest is
/// reported as `remaining`, never dropped.
pub const BI_CHECK_CAP: usize = 500;

pub(crate) fn router() -> Router<CpState> {
    Router::new().route("/classifications/{id}/usage", get(tag_usage))
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(tag_usage),
    components(schemas(
        TagUsage, UsageTag, DataUsage, TableUsage, BiUsage, BiObject, Unreadable
    ))
)]
pub(crate) struct ControlPlaneTagUsageApi;

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UsageTag {
    pub id: Uuid,
    pub key: String,
    pub name: String,
}

/// One table of an egret catalog carrying the tag, on the table and/or on
/// some of its columns.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TableUsage {
    pub catalog_id: Uuid,
    pub catalog_name: String,
    pub schema: String,
    pub table: String,
    /// The tag is on the table itself.
    pub on_table: bool,
    /// Tagged columns, sorted.
    pub columns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DataUsage {
    /// Sorted by catalog name, schema, table.
    pub tables: Vec<TableUsage>,
    /// Tables carrying the tag on the table itself.
    pub table_count: i64,
    /// Tagged columns.
    pub column_count: i64,
    /// Assignments in a warehouse no catalog names (CL11); not in `tables`.
    pub orphaned: i64,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BiObject {
    /// `dashboard`, `chart`, `dataset`, `saved_query`, `app` or `folder`.
    pub kind: String,
    pub id: Uuid,
    pub name: String,
    pub workspace_slug: String,
}

/// Objects the caller cannot read: a count, and nothing else (Q5).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Unreadable {
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BiUsage {
    /// Readable objects, in check order (workspace id, kind, object id).
    pub readable: Vec<BiObject>,
    pub unreadable: Unreadable,
    /// Assignments whose object no longer exists (CL11).
    pub deleted: i64,
    /// Objects checked, at most [`BI_CHECK_CAP`].
    pub checked: i64,
    /// Existing objects beyond the cap, not checked (CL12).
    pub remaining: i64,
}

/// Route C's answer.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct TagUsage {
    pub tag: UsageTag,
    pub data: DataUsage,
    pub bi: BiUsage,
}

/// A catalog assignment and the catalog naming its warehouse, if any.
type AssignmentRow = (Option<Uuid>, Option<String>, String, String, String);

fn db(what: &'static str) -> impl FnOnce(sqlx::Error) -> CpError {
    move |e| {
        tracing::error!(error = %e, "classification usage: {what}");
        CpError::Internal
    }
}

/// Where a classification is used.
#[utoipa::path(
    get,
    path = "/api/control-plane/classifications/{id}/usage",
    tag = "control-plane-governance",
    params(("id" = Uuid, Path, description = "Tag id")),
    description = "The tables and columns of egret catalogs carrying the classification (grouped by \
                   table; assignments in a warehouse no catalog names are counted as `orphaned`), \
                   and the BI objects carrying it: each is checked with the caller's read \
                   permission, in workspace, kind, id order, up to 500 per request; readable ones \
                   are listed, unreadable ones only counted (no name, no workspace), the rest \
                   reported as `remaining`, and assignments to deleted objects counted. Requires \
                   tag:read or policy:read.",
    responses(
        (status = 200, description = "Where the classification is used", body = TagUsage),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Lacks both tag:read and policy:read", body = ErrorBody),
        (status = 404, description = "No tag with this id", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    )
)]
async fn tag_usage(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<TagUsage>, CpError> {
    require_governance_read(&st, &p).await?;
    let pool = &st.db.pool;
    let tag: Option<(Uuid, String, String)> =
        sqlx::query_as("SELECT id, key, name FROM tags WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await
            .map_err(db("read the tag"))?;
    let Some((id, key, name)) = tag else {
        return Err(CpError::NotFound);
    };

    // ── Data ──
    let rows: Vec<AssignmentRow> = sqlx::query_as(
        "SELECT c.id, c.name, a.schema_name, a.table_name, a.column_name \
           FROM catalog_tag_assignments a \
           LEFT JOIN registry.catalogs c ON c.warehouse_id = a.warehouse_id \
          WHERE a.tag_id = $1",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .map_err(db("read the catalog assignments"))?;
    let mut tables: BTreeMap<(String, Uuid, String, String), TableUsage> = BTreeMap::new();
    let (mut table_count, mut column_count, mut orphaned) = (0i64, 0i64, 0i64);
    for (cid, cname, schema, table, column) in rows {
        let (Some(cid), Some(cname)) = (cid, cname) else {
            orphaned += 1;
            continue;
        };
        let entry = tables
            .entry((cname.clone(), cid, schema.clone(), table.clone()))
            .or_insert_with(|| TableUsage {
                catalog_id: cid,
                catalog_name: cname,
                schema,
                table,
                on_table: false,
                columns: Vec::new(),
            });
        if column.is_empty() {
            entry.on_table = true;
            table_count += 1;
        } else {
            entry.columns.push(column);
            column_count += 1;
        }
    }
    let tables: Vec<TableUsage> = tables
        .into_values()
        .map(|mut t| {
            t.columns.sort();
            t
        })
        .collect();

    // ── BI ──
    // Each assignment's workspace, so the checks run in a fixed order; a NULL
    // workspace is an object that no longer exists.
    let objects: Vec<(String, Uuid, Option<Uuid>)> = sqlx::query_as(
        "SELECT ta.object_type, ta.object_id, w.workspace_id \
           FROM tag_assignments ta \
           LEFT JOIN LATERAL ( \
                SELECT workspace_id FROM analytics.dashboards WHERE ta.object_type = 'dashboard' AND id = ta.object_id \
                UNION ALL SELECT workspace_id FROM analytics.charts WHERE ta.object_type = 'chart' AND id = ta.object_id \
                UNION ALL SELECT workspace_id FROM analytics.datasets WHERE ta.object_type = 'dataset' AND id = ta.object_id \
                UNION ALL SELECT workspace_id FROM analytics.saved_queries WHERE ta.object_type = 'saved_query' AND id = ta.object_id \
                UNION ALL SELECT workspace_id FROM analytics.apps WHERE ta.object_type = 'app' AND id = ta.object_id \
                UNION ALL SELECT workspace_id FROM analytics.folders WHERE ta.object_type = 'folder' AND id = ta.object_id \
           ) w ON true \
          WHERE ta.tag_id = $1 \
          ORDER BY w.workspace_id, ta.object_type, ta.object_id",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .map_err(db("read the BI assignments"))?;
    let mut deleted = 0i64;
    let mut existing = Vec::new();
    for (kind, oid, ws) in objects {
        match ws {
            Some(_) => existing.push((kind, oid)),
            None => deleted += 1,
        }
    }
    let remaining = existing.len().saturating_sub(BI_CHECK_CAP) as i64;
    existing.truncate(BI_CHECK_CAP);
    let subject = p.subject();
    let mut readable = Vec::new();
    let mut unreadable = 0i64;
    let mut checked = 0i64;
    let mut slugs: BTreeMap<Uuid, String> = BTreeMap::new();
    for (kind, oid) in existing {
        let object = match egret_bi::shares::object_ref(&st.db, &kind, oid).await {
            Ok(o) => o,
            // Deleted between the read above and this one.
            Err(egret_core::Error::NotFound(_)) => {
                deleted += 1;
                continue;
            }
            // A kind no share knows: nobody can be shown it.
            Err(egret_core::Error::InvalidInput(_)) => {
                checked += 1;
                unreadable += 1;
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        checked += 1;
        let action = match egret_bi::shares::object_gate_kind(&st.db, &kind, oid).await {
            Ok(gate) => egret_bi::shares::read_action_for_gate(&gate),
            Err(egret_core::Error::NotFound(_)) | Err(egret_core::Error::InvalidInput(_)) => None,
            Err(e) => return Err(e.into()),
        };
        let Some(action) = action else {
            unreadable += 1;
            continue;
        };
        let ws = object.workspace_id;
        let allowed = st
            .pdp
            .authorize(&subject, action, &Resource::Object(object))
            .await?
            .allow;
        if !allowed {
            unreadable += 1;
            continue;
        }
        let Some(name) = egret_bi::shares::object_name(&st.db, &kind, oid).await? else {
            deleted += 1;
            checked -= 1;
            continue;
        };
        let slug = match slugs.get(&ws.0) {
            Some(slug) => slug.clone(),
            None => {
                let slug: String = sqlx::query_scalar("SELECT slug FROM workspaces WHERE id = $1")
                    .bind(ws.0)
                    .fetch_one(pool)
                    .await
                    .map_err(db("read a workspace slug"))?;
                slugs.insert(ws.0, slug.clone());
                slug
            }
        };
        readable.push(BiObject {
            kind,
            id: oid,
            name,
            workspace_slug: slug,
        });
    }

    Ok(Json(TagUsage {
        tag: UsageTag { id, key, name },
        data: DataUsage {
            tables,
            table_count,
            column_count,
            orphaned,
        },
        bi: BiUsage {
            readable,
            unreadable: Unreadable { count: unreadable },
            deleted,
            checked,
            remaining,
        },
    }))
}
