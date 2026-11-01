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

use egret_perm_compile::table_columns::{
    missing_columns_via, table_states_via, TableKey, TableState,
};

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
        TagUsage,
        UsageTag,
        DataUsage,
        TableUsage,
        TableState,
        MissingColumn,
        BiUsage,
        BiObject,
        Unreadable
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
    /// The warehouse the assignment is stored under (the catalog's
    /// `warehouse_id`): what a write to this table's classification names.
    pub warehouse_id: Uuid,
    pub schema: String,
    pub table: String,
    /// The tag is on the table itself.
    pub on_table: bool,
    /// Tagged columns, sorted.
    pub columns: Vec<String>,
    /// What the embedded catalog holds under the table's name (rename spec
    /// C15): `live`, `dropped` (only soft-deleted, an undrop restores it) or
    /// `missing` (renamed away, purged, or classified before it existed).
    /// Absent when the warehouse is not an embedded catalog.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<TableState>,
    /// Tagged columns a live table does not have, sorted; absent when none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub missing_columns: Vec<MissingColumn>,
}

/// A tagged column its live table does not have (rename spec R8).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MissingColumn {
    pub column: String,
    /// The current name of the field that last carried this name, when it is
    /// still in the table: a candidate a steward confirms, never applied.
    pub renamed_to: Option<String>,
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
type AssignmentRow = (Option<Uuid>, Option<String>, Uuid, String, String, String);

fn db(what: &'static str) -> impl FnOnce(sqlx::Error) -> CpError {
    move |e| {
        tracing::error!(error = %e, "classification usage: {what}");
        CpError::Internal
    }
}

/// Fill in each table's [`TableState`] and missing columns, for the tables
/// of warehouses the embedded catalog serves: one state query and one schema
/// query per request.
async fn table_states(
    conn: &mut sqlx::PgConnection,
    tables: &mut [TableUsage],
) -> Result<(), egret_perm::CompileError> {
    let warehouses: Vec<Uuid> = tables.iter().map(|t| t.warehouse_id).collect();
    let embedded: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM catalog.warehouses \
          WHERE id = ANY($1) AND lakekeeper_warehouse_id IS NOT NULL",
    )
    .bind(&warehouses)
    .fetch_all(&mut *conn)
    .await
    .map_err(egret_perm::CompileError::from_db)?;
    let at: Vec<usize> = (0..tables.len())
        .filter(|&i| embedded.contains(&tables[i].warehouse_id))
        .collect();
    if at.is_empty() {
        return Ok(());
    }
    let key = |t: &TableUsage| TableKey {
        warehouse_id: t.warehouse_id,
        schema: t.schema.clone(),
        table: t.table.clone(),
    };
    let keys: Vec<TableKey> = at.iter().map(|&i| key(&tables[i])).collect();
    let states = table_states_via(conn, &keys).await?;
    let mut asked = Vec::new();
    for (&i, state) in at.iter().zip(states) {
        tables[i].state = Some(state);
        if state == TableState::Live && !tables[i].columns.is_empty() {
            asked.push((key(&tables[i]), tables[i].columns.clone()));
        }
    }
    for m in missing_columns_via(conn, &asked).await? {
        if let Some(t) = tables.iter_mut().find(|t| key(t) == m.key) {
            t.missing_columns.push(MissingColumn {
                column: m.column,
                renamed_to: m.renamed_to,
            });
        }
    }
    for t in tables.iter_mut() {
        t.missing_columns.sort_by(|a, b| a.column.cmp(&b.column));
    }
    Ok(())
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
                   reported as `remaining`, and assignments to deleted objects counted. Each \
                   table of an embedded catalog says whether the catalog has it (`state`: live, \
                   dropped or missing) and which of its tagged columns it lacks, with the \
                   column's likely new name (`missingColumns`). Requires tag:read or \
                   policy:read.",
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
        "SELECT c.id, c.name, a.warehouse_id, a.schema_name, a.table_name, a.column_name \
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
    for (cid, cname, warehouse_id, schema, table, column) in rows {
        let (Some(cid), Some(cname)) = (cid, cname) else {
            orphaned += 1;
            continue;
        };
        let entry = tables
            .entry((cname.clone(), cid, schema.clone(), table.clone()))
            .or_insert_with(|| TableUsage {
                catalog_id: cid,
                catalog_name: cname,
                warehouse_id,
                schema,
                table,
                on_table: false,
                columns: Vec::new(),
                state: None,
                missing_columns: Vec::new(),
            });
        if column.is_empty() {
            entry.on_table = true;
            table_count += 1;
        } else {
            entry.columns.push(column);
            column_count += 1;
        }
    }
    let mut tables: Vec<TableUsage> = tables
        .into_values()
        .map(|mut t| {
            t.columns.sort();
            t
        })
        .collect();
    let mut conn = pool.acquire().await.map_err(db("acquire"))?;
    table_states(&mut conn, &mut tables).await.map_err(|e| {
        tracing::error!(error = %e, "classification usage: read the table states");
        CpError::Internal
    })?;

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
