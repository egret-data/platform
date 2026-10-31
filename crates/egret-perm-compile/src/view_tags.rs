//! View governance B3: keep a view's column tags in step with its base
//! tables' classifications.
//!
//! A view's **lineage-derived** tags are, by spec R3, every clearance-bearing
//! tag of every table in its recorded lineage ([`lineage::view_tag_set`]), on
//! every column of the view. This module rewrites them so that they are
//! exactly that set:
//!
//! - when a base column is classified, reclassified or declassified, inside
//!   the transaction of that write, under [`lock_view_tags_in_tx`]
//!   ([`dependent_views`], then [`retag_views_in_tx`]);
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

fn db_err(e: sqlx::Error) -> CompileError {
    CompileError::from_db(e)
}

/// The one lock every writer of view tag rows or view lineage takes as the
/// FIRST statement of its transaction (Task 9 fix round 1, I2): the base
/// classification write (`egret_bi::CatalogTagRepo`, `ClassificationPolicyRepo`),
/// the commit hold's write, its compensation and its after-commit record, the
/// drop and rename cleanups, the sweep's record, and the undrop check.
///
/// Without it two transactions skew under READ COMMITTED: the hold reads the
/// view's tag set before a base classification commits, while the base write
/// reads the dependent views before the hold's lineage commits. Each sees
/// neither change, and the view commits without the new tag. With it the
/// writers serialise, and each reads the other's committed result.
///
/// A transaction-scoped advisory lock, taken first and released at commit or
/// rollback. No writer holds a row lock when it waits on it, and nothing
/// that waits on a row lock a writer holds ever takes it, so it adds no
/// deadlock cycle. Tag writes are admin operations and holds are capped (8),
/// so serialising them costs little.
pub async fn lock_view_tags_in_tx(conn: &mut sqlx::PgConnection) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('egret-view-tags', 0))")
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// The advisory-lock key of one table or view NAME of a Lakekeeper warehouse
/// (`prefix`): the view hold's and the table hook's name locks, and the
/// catalog-tag write's try-lock, all use it (rename spec C17, review findings
/// 2 and 6). Folded, as Lakekeeper's names are case-insensitive, so `Orders`
/// and `orders` are one lock.
pub fn name_lock_key(prefix: &str, ns: &str, name: &str) -> String {
    // A NUL cannot be sent as Postgres text; `\u{1e}` occurs in no name Trino
    // sends.
    format!(
        "egret-view-hold\u{1e}{}\u{1e}{}\u{1e}{}",
        prefix.to_lowercase(),
        ns.to_lowercase(),
        name.to_lowercase()
    )
}

/// Try, on the caller's transaction, the name lock of `schema.table` in
/// `warehouse_id` ([`name_lock_key`]), released at commit or rollback. `false`
/// while a hold (a table move or a view hold) holds the name; `true` when it
/// is free or the warehouse is not an embedded catalog. A catalog-tag write
/// takes it so a write made while a rename of the table is held does not land
/// on a name the move is about to leave (review finding 2).
pub async fn try_name_lock_in_tx(
    conn: &mut sqlx::PgConnection,
    warehouse_id: Uuid,
    schema: &str,
    table: &str,
) -> Result<bool, sqlx::Error> {
    let prefix: Option<Option<String>> =
        sqlx::query_scalar("SELECT lakekeeper_warehouse_id FROM catalog.warehouses WHERE id = $1")
            .bind(warehouse_id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(prefix) = prefix.flatten() else {
        return Ok(true);
    };
    sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(name_lock_key(&prefix, schema, table))
        .fetch_one(&mut *conn)
        .await
}

/// The `LISTEN` channel a classification write notifies after it commits
/// (`egret_catalog_engine::spawn_classification_sweeps`).
pub const CLASSIFICATION_CHANNEL: &str = "egret_view_lineage_sweep";

/// The payload that asks for every warehouse.
pub const CLASSIFICATION_ALL: &str = "*";

/// Ask, on the caller's transaction, for a sweep of the views with no
/// computed lineage that `warehouse_ids`' profiles reach. Postgres delivers a
/// `NOTIFY` only when the transaction commits, so the sweep runs after the
/// classification is visible, and never for a write that rolled back. A list
/// too long for one payload asks for every warehouse instead.
pub async fn notify_classification_in_tx(
    conn: &mut sqlx::PgConnection,
    warehouse_ids: &[Uuid],
) -> Result<(), sqlx::Error> {
    if warehouse_ids.is_empty() {
        return Ok(());
    }
    // A NOTIFY payload must stay under 8000 bytes; 37 bytes per id.
    let payload = if warehouse_ids.len() > 200 {
        CLASSIFICATION_ALL.to_string()
    } else {
        warehouse_ids
            .iter()
            .map(Uuid::to_string)
            .collect::<Vec<_>>()
            .join(",")
    };
    sqlx::query("SELECT pg_notify($1, $2)")
        .bind(CLASSIFICATION_CHANNEL)
        .bind(payload)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// The warehouses of `views`, ascending.
pub async fn warehouses_of_views(
    conn: &mut sqlx::PgConnection,
    views: &[ViewRef],
) -> Result<Vec<Uuid>, sqlx::Error> {
    let catalogs: BTreeSet<&str> = views.iter().map(|v| v.catalog.as_str()).collect();
    let mut out = BTreeSet::new();
    for catalog in catalogs {
        if let Some(w) = warehouse_of(conn, catalog).await? {
            out.insert(w);
        }
    }
    Ok(out.into_iter().collect())
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
    warehouses_of_views(conn, &views).await.map_err(db_err)
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
        .map_err(crate::db_error::from_lineage)?
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

    // M2: rows are written in one order, (tag, column) ascending, here and in
    // the hold, so two writers on the same coordinates take their row locks
    // in the same order. (The lock above serialises them anyway.)
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
          ORDER BY t.id, c.name \
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

/// Why [`ViewReach`] refused, typed so each caller keeps its own mapping (the
/// catalog-tag routes answer a database error, a compile refusal and a view
/// that changed underneath differently).
#[derive(Debug, thiserror::Error)]
pub enum ViewReachError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Compile(#[from] CompileError),
    /// A re-tag touched a warehouse the reach did not lock.
    #[error("a view in warehouse {0} changed while this tag write ran; retry")]
    ViewChanged(Uuid),
}

/// What a catalog-tag write on some tables of one warehouse reaches (view
/// governance B3): the written warehouse, the views whose lineage reads those
/// tables and their warehouses, and the services bound to any of them.
/// Computed ONCE, before the write and after [`lock_view_tags_in_tx`], so no
/// lineage can commit in between (Task 9 fix round 1, I2), and the overlap
/// snapshot covers every profile the write can recompile. Shared by the
/// catalog-tag write (`egret_bi::CatalogTagRepo`) and the table hook's move
/// (`egret_catalog_engine::table_hook`), so the lock order is spelled once.
pub struct ViewReach {
    pub warehouse_id: Uuid,
    pub views: Vec<ViewRef>,
    pub warehouses: Vec<Uuid>,
    pub services: Vec<i64>,
}

impl ViewReach {
    /// [`Self::of_tables`] for one table.
    pub async fn of(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        warehouse_id: Uuid,
        schema: &str,
        table: &str,
    ) -> Result<Self, ViewReachError> {
        Self::of_tables(tx, warehouse_id, &[(schema.to_string(), table.to_string())]).await
    }

    /// Takes the view-tag lock, so call it FIRST in the transaction.
    pub async fn of_tables(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        warehouse_id: Uuid,
        tables: &[(String, String)],
    ) -> Result<Self, ViewReachError> {
        lock_view_tags_in_tx(tx).await?;
        let mut lineage_tables = Vec::new();
        for (schema, table) in tables {
            lineage_tables.extend(tables_of_warehouse(tx, warehouse_id, schema, table).await?);
        }
        let views = dependent_views(tx, &lineage_tables).await?;
        let mut warehouses = warehouses_of_views(tx, &views).await?;
        warehouses.push(warehouse_id);
        warehouses.sort();
        warehouses.dedup();
        let services =
            egret_catalog_registry::services_bound_to_warehouses(&mut **tx, &warehouses).await?;
        // Before the base write, whose `assigned_by` holds `FOR KEY SHARE` on
        // the assigning user: the profile locks, then the compile-mint lock
        // the recompile mints under. Taken after that write, a concurrent
        // compile minting the same user would wait on this transaction while
        // it waited on that compile (`crate::lock_compile_mints_in_tx`).
        // The recompile's own set is a subset of `services` (its warehouses
        // are, `retag_bump_and_recompile` refuses otherwise), so it re-takes
        // only locks already held.
        if !services.is_empty() {
            crate::lock_services_in_tx(tx, &services).await?;
            crate::lock_compile_mints_in_tx(tx).await?;
        }
        Ok(Self {
            warehouse_id,
            views,
            warehouses,
            services,
        })
    }

    /// After the base write: re-tag the dependent views, then ONE bump over
    /// the written warehouse and the warehouses the re-tag actually changed,
    /// one recompile of their services, and a request, delivered at commit,
    /// for the sweep of the views that have no computed lineage yet.
    pub async fn retag_bump_and_recompile(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), ViewReachError> {
        let summary = retag_views_in_tx(tx, &self.views).await?;
        let mut touched: Vec<Uuid> = summary.warehouses.into_iter().collect();
        // The lock keeps the views from changing since `of`, so the re-tag
        // stays inside the snapshot. Refuse rather than leave a write the
        // snapshot and the bump did not cover.
        if let Some(outside) = touched.iter().find(|w| !self.warehouses.contains(w)) {
            return Err(ViewReachError::ViewChanged(*outside));
        }
        touched.push(self.warehouse_id);
        touched.sort();
        touched.dedup();
        let services =
            egret_catalog_registry::services_bound_to_warehouses(&mut **tx, &touched).await?;
        egret_catalog_registry::bump_tag_versions_for_warehouses(tx, &touched).await?;
        recompile_bound_services_in_tx(tx, &touched, &services).await?;
        notify_classification_in_tx(tx, &[self.warehouse_id]).await?;
        Ok(())
    }
}

/// Recompile every policy profile the written warehouses' engines are bound
/// through, INSIDE the tag write's own transaction (SP4a Task 7, D5; fix round
/// 1): a catalog-tag write can add or remove a column from a StarRocks tag
/// rule's compile-time enumeration, StarRocks has no tag service to resolve
/// that itself, and a recompile FAILURE rolls the write back with it. A skip
/// is not a failure; it is logged once more per service with the warehouses,
/// since only the write knows which one triggered it.
async fn recompile_bound_services_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    warehouse_ids: &[Uuid],
    service_ids: &[i64],
) -> Result<(), CompileError> {
    // SP4a Task 11, defect 2: a recompile also locks and writes the profile's
    // TAG service, whose id can be lower than the profile's. Take the whole
    // expanded lock set ascending up front (`lock_services_in_tx`).
    crate::lock_services_in_tx(tx, service_ids).await?;
    for &service_id in service_ids {
        let summary = crate::recompile_service_in_tx(tx, service_id).await?;
        if !summary.skipped.is_empty() {
            tracing::warn!(
                warehouse_ids = ?warehouse_ids,
                service_id,
                skipped_count = summary.skipped.len(),
                "catalog-tag write's recompile skipped some grants/rules for this service"
            );
        }
    }
    Ok(())
}
