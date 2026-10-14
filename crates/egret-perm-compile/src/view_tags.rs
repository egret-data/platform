//! View governance B3: keep a view's column tags in step with its base
//! tables' classifications.
//!
//! A view's **lineage-derived** tags are, by spec R3, every clearance-bearing
//! tag of every table in its recorded lineage ([`lineage::view_tag_set`]), on
//! every column of the view. This module rewrites them so that they are
//! exactly that set:
//!
//! - when a base column is classified, reclassified or declassified, inside
//!   the transaction of that write ([`retag_dependent_views_in_tx`]);
//! - when the B3 sweep recomputes a view's lineage
//!   (`egret_catalog_engine::lineage_sweep`), for that one view
//!   ([`retag_view_in_tx`]).
//!
//! # What is never touched
//!
//! - **A view whose lineage is not computed** (missing or failed): its tags stay
//!   as they are. Tags are never removed on uncertainty (spec B3).
//! - **A tag an admin put on a view column by hand.** Only rows with
//!   `from_lineage` (migration 0188) are removed or written here. A derived
//!   row that an admin assigns again becomes manual and is kept.
//!
//! # Removals included
//!
//! A computed lineage is what the engine reported, so the derived set is
//! replaced, not only grown: a declassified base takes its tag off the view,
//! and a `CREATE OR REPLACE` that narrowed the view (Task 8 M6: the hold only
//! adds rows) loses the tags its old SQL carried.
//!
//! # No bump here
//!
//! Nothing here bumps a tag version or recompiles. The caller does both ONCE,
//! over the base's warehouse and [`dependent_view_warehouses`] together, so a
//! base change and its dependent views reach every plugin in one tag version.

use std::collections::BTreeSet;

use egret_perm::CompileError;
use uuid::Uuid;

use crate::lineage::{self, LineageTable, RecordedLineage, ViewRef};

fn db_err<E: std::fmt::Display>(e: E) -> CompileError {
    CompileError::Db(e.to_string())
}

/// What a retag did to one view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewRetag {
    /// The derived rows already were the derived set.
    Unchanged,
    /// Rows were written or removed in `warehouse_id`.
    Changed { warehouse_id: Uuid },
    /// Nothing was touched, and why: the lineage is missing or failed, the
    /// view's catalog is unknown, or its columns are not recorded.
    Kept { reason: String },
}

/// What [`retag_views_in_tx`] did.
#[derive(Debug, Default, Clone)]
pub struct RetagSummary {
    /// The views whose derived rows changed.
    pub changed: Vec<ViewRef>,
    /// The warehouses those rows are in.
    pub warehouses: BTreeSet<Uuid>,
    /// The views left as they were, with the reason.
    pub kept: Vec<(ViewRef, String)>,
}

/// The `(catalog, schema, table)` names under which the lineage records
/// `warehouse_id`'s table `schema.table`: one per registry catalog over the
/// warehouse, since the lineage uses the engine's catalog name
/// (`registry.catalogs.name`).
pub async fn tables_of_warehouse(
    conn: &mut sqlx::PgConnection,
    warehouse_id: Uuid,
    schema: &str,
    table: &str,
) -> Result<Vec<LineageTable>, sqlx::Error> {
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM registry.catalogs WHERE warehouse_id = $1 ORDER BY name",
    )
    .bind(warehouse_id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(names
        .into_iter()
        .map(|c| (c, schema.to_string(), table.to_string()))
        .collect())
}

/// Every table that carries `tag_id` on any of its columns (or on the table
/// itself), under its registry catalog names: what a change to the tag's
/// clearance can change for a view.
pub async fn tables_carrying_tag(
    conn: &mut sqlx::PgConnection,
    tag_id: Uuid,
) -> Result<Vec<LineageTable>, sqlx::Error> {
    sqlx::query_as(
        "SELECT DISTINCT c.name, a.schema_name, a.table_name \
           FROM catalog_tag_assignments a \
           JOIN registry.catalogs c ON c.warehouse_id = a.warehouse_id \
          WHERE a.tag_id = $1 \
          ORDER BY 1, 2, 3",
    )
    .bind(tag_id)
    .fetch_all(&mut *conn)
    .await
}

/// The views whose recorded lineage contains any of `changed`. A lineage row
/// exists only for a computed lineage (`record_lineage_failure` removes the
/// rows), so a failed or missing view is never among them. The schema and
/// table are case-folded, as [`lineage::view_tag_set`] matches them.
pub async fn dependent_views(
    conn: &mut sqlx::PgConnection,
    changed: &[LineageTable],
) -> Result<Vec<ViewRef>, sqlx::Error> {
    if changed.is_empty() {
        return Ok(Vec::new());
    }
    let (catalogs, (schemas, names)): (Vec<&str>, (Vec<&str>, Vec<&str>)) = changed
        .iter()
        .map(|(c, s, t)| (c.as_str(), (s.as_str(), t.as_str())))
        .unzip();
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT DISTINCT l.view_catalog, l.view_schema, l.view_name \
           FROM view_lineage l \
           JOIN unnest($1::text[], $2::text[], $3::text[]) AS c(catalog, schema_name, table_name) \
             ON l.base_catalog = c.catalog \
            AND lower(l.base_schema) = lower(c.schema_name) \
            AND lower(l.base_table) = lower(c.table_name) \
          ORDER BY 1, 2, 3",
    )
    .bind(&catalogs)
    .bind(&schemas)
    .bind(&names)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(catalog, schema, name)| ViewRef {
            catalog,
            schema,
            name,
        })
        .collect())
}

/// The warehouse a view's catalog name is a registry catalog of.
async fn warehouse_of(
    conn: &mut sqlx::PgConnection,
    catalog: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    let found: Option<Option<Uuid>> =
        sqlx::query_scalar("SELECT warehouse_id FROM registry.catalogs WHERE name = $1")
            .bind(catalog)
            .fetch_optional(&mut *conn)
            .await?;
    Ok(found.flatten())
}

/// The warehouses holding the views that depend on `changed`, ascending. A
/// caller bumps and recompiles these together with the base's own warehouse,
/// once ([module doc](self)).
pub async fn dependent_view_warehouses(
    conn: &mut sqlx::PgConnection,
    changed: &[LineageTable],
) -> Result<Vec<Uuid>, CompileError> {
    let views = dependent_views(conn, changed).await.map_err(db_err)?;
    let catalogs: BTreeSet<&str> = views.iter().map(|v| v.catalog.as_str()).collect();
    let mut out = BTreeSet::new();
    for catalog in catalogs {
        if let Some(w) = warehouse_of(conn, catalog).await.map_err(db_err)? {
            out.insert(w);
        }
    }
    Ok(out.into_iter().collect())
}

/// Rewrite `view`'s lineage-derived rows to exactly its derived set
/// ([module doc](self)). Runs on the caller's connection, so it joins the
/// caller's transaction; it never bumps.
pub async fn retag_view_in_tx(
    conn: &mut sqlx::PgConnection,
    view: &ViewRef,
) -> Result<ViewRetag, CompileError> {
    let tables = match lineage::recorded_lineage(conn, view)
        .await
        .map_err(db_err)?
    {
        RecordedLineage::Computed(t) if !t.is_empty() => t,
        RecordedLineage::Computed(_) => {
            return Ok(ViewRetag::Kept {
                reason: "the recorded lineage is empty".into(),
            })
        }
        RecordedLineage::Missing => {
            return Ok(ViewRetag::Kept {
                reason: "no lineage is recorded".into(),
            })
        }
        RecordedLineage::Failed { detail } => {
            return Ok(ViewRetag::Kept {
                reason: format!("the lineage could not be computed: {detail}"),
            })
        }
    };
    let Some(warehouse_id) = warehouse_of(conn, &view.catalog).await.map_err(db_err)? else {
        return Ok(ViewRetag::Kept {
            reason: format!("no registry catalog is named {:?}", view.catalog),
        });
    };
    let tags: Vec<Uuid> = lineage::view_tag_set(conn, &tables)
        .await
        .map_err(db_err)?
        .into_iter()
        .collect();

    // The columns: as recorded with the lineage, else the columns the derived
    // rows already cover. With tags to write and no column known, nothing is
    // touched: removing would be removal on uncertainty, and writing needs
    // the columns. The sweep loads them from the catalog.
    let columns: Vec<String> = match lineage::recorded_view_columns(conn, view)
        .await
        .map_err(db_err)?
    {
        Some(c) => c,
        None => sqlx::query_scalar(
            "SELECT DISTINCT column_name FROM catalog_tag_assignments \
              WHERE warehouse_id = $1 AND schema_name = $2 AND table_name = $3 \
                AND from_lineage ORDER BY 1",
        )
        .bind(warehouse_id)
        .bind(&view.schema)
        .bind(&view.name)
        .fetch_all(&mut *conn)
        .await
        .map_err(db_err)?,
    };
    if columns.is_empty() && !tags.is_empty() {
        return Ok(ViewRetag::Kept {
            reason: "the view's columns are not recorded; the next lineage sweep records them"
                .into(),
        });
    }

    let removed = sqlx::query(
        "DELETE FROM catalog_tag_assignments a \
          WHERE a.warehouse_id = $1 AND a.schema_name = $2 AND a.table_name = $3 \
            AND a.from_lineage \
            AND NOT (a.tag_id = ANY($4::uuid[]) AND a.column_name = ANY($5::text[]))",
    )
    .bind(warehouse_id)
    .bind(&view.schema)
    .bind(&view.name)
    .bind(&tags)
    .bind(&columns)
    .execute(&mut *conn)
    .await
    .map_err(db_err)?
    .rows_affected();
    let added = sqlx::query(
        "INSERT INTO catalog_tag_assignments \
             (tag_id, warehouse_id, schema_name, table_name, column_name, from_lineage) \
         SELECT t.id, $1, $2, $3, c.name, true \
           FROM unnest($4::uuid[]) AS t(id) CROSS JOIN unnest($5::text[]) AS c(name) \
         ON CONFLICT (tag_id, warehouse_id, schema_name, table_name, column_name) DO NOTHING",
    )
    .bind(warehouse_id)
    .bind(&view.schema)
    .bind(&view.name)
    .bind(&tags)
    .bind(&columns)
    .execute(&mut *conn)
    .await
    .map_err(db_err)?
    .rows_affected();
    Ok(if removed + added > 0 {
        ViewRetag::Changed { warehouse_id }
    } else {
        ViewRetag::Unchanged
    })
}

/// [`retag_view_in_tx`] for each of `views`.
pub async fn retag_views_in_tx(
    conn: &mut sqlx::PgConnection,
    views: &[ViewRef],
) -> Result<RetagSummary, CompileError> {
    let mut summary = RetagSummary::default();
    for view in views {
        match retag_view_in_tx(conn, view).await? {
            ViewRetag::Unchanged => {}
            ViewRetag::Changed { warehouse_id } => {
                summary.changed.push(view.clone());
                summary.warehouses.insert(warehouse_id);
            }
            ViewRetag::Kept { reason } => {
                tracing::info!(?view, %reason, "view retag: tags kept as they are");
                summary.kept.push((view.clone(), reason));
            }
        }
    }
    Ok(summary)
}

/// Re-tag every view whose lineage contains one of `changed_tables`, on the
/// caller's open transaction, after the base write. Returns how many views'
/// rows changed. Does not bump or recompile: the caller does both once over
/// the base's warehouse and [`dependent_view_warehouses`] ([module doc](self)).
pub async fn retag_dependent_views_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    changed_tables: &[(String, String, String)],
) -> Result<usize, CompileError> {
    Ok(retag_dependent_views_detailed_in_tx(tx, changed_tables)
        .await?
        .changed
        .len())
}

/// [`retag_dependent_views_in_tx`], with what it did.
pub async fn retag_dependent_views_detailed_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    changed_tables: &[(String, String, String)],
) -> Result<RetagSummary, CompileError> {
    let views = dependent_views(tx, changed_tables).await.map_err(db_err)?;
    retag_views_in_tx(tx, &views).await
}
