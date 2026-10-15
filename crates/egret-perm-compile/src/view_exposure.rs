//! View governance A4 (revised 2026-10-14): which classified tables each
//! StarRocks engine reaches, so the control plane can find the materialized
//! views that already read one of them.
//!
//! The A3 deny (`crate::clearance::mv_deny_inputs`) stops NEW materialized
//! views. It cannot remove an existing one, and an existing one may hold a copy
//! of the classified values. StarRocks keeps every materialized view in
//! [`MV_CATALOG`] (measured, `docs/en/plans/2026-10-14-starrocks-mv-placement-
//! measurement.md`), whatever catalog its base tables live in, so the control
//! plane lists `information_schema.materialized_views` once per engine and
//! matches each view's base tables against the set this module returns.
//!
//! **Per engine, from that engine's OWN catalog attachments.** Two engines may
//! share one policy profile, and the profile-wide query
//! (`egret_policy::tagged_columns_for_service_via`) returns every column
//! reachable through ANY engine on the profile. A view on engine B can only
//! read a catalog attached to B, so each engine is matched against the
//! classified tables of its own attachments (Task 4 review I-3).

use sqlx::PgConnection;
use uuid::Uuid;

use egret_perm::CompileError;

/// The only catalog StarRocks creates a materialized view in.
pub use crate::clearance::MV_CATALOG;

/// A table with at least one column of the classification, as the engine names
/// it: `catalog` is the registry catalog name the engine attaches it under.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ClassifiedTable {
    pub catalog: String,
    pub database: String,
    pub table: String,
}

/// A StarRocks engine bound to a policy profile that reaches at least one
/// table of the classification through its own catalog attachments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedEngine {
    pub engine_id: Uuid,
    pub engine_name: String,
    /// `engine_mgmt.engines.status`: only a `running` engine is dialled.
    pub status: String,
    /// `engine_mgmt.engines.origin`: `provisioned` or `external`. An external
    /// engine is stored `running` (a placeholder, migration 0184) but egret
    /// never connects to one and holds no credential for it.
    pub origin: String,
    /// Sorted, without duplicates.
    pub tables: Vec<ClassifiedTable>,
}

#[derive(sqlx::FromRow)]
struct Row {
    engine_id: Uuid,
    engine_name: String,
    status: String,
    origin: String,
    catalog: String,
    schema_name: String,
    table_name: String,
}

/// Every StarRocks engine bound to a policy profile, with the tables tagged
/// `tag_key` in the catalogs attached to THAT engine, ordered by engine name.
/// An engine that reaches none is absent. Column-level assignments only, as the
/// clearance compiles them (`a.column_name <> ''`, the same filter as
/// `tagged_columns_for_service_via`).
pub async fn classified_starrocks_engines(
    conn: &mut PgConnection,
    tag_key: &str,
) -> Result<Vec<ClassifiedEngine>, CompileError> {
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT DISTINCT e.id AS engine_id, e.name AS engine_name, e.status, e.origin, \
                c.name AS catalog, a.schema_name, a.table_name \
           FROM engine_mgmt.engines e \
           JOIN registry.engine_catalog_attachments ea ON ea.engine_id = e.id \
           JOIN registry.catalogs c ON c.id = ea.catalog_id \
           JOIN catalog_tag_assignments a ON a.warehouse_id = c.warehouse_id \
           JOIN tags t ON t.id = a.tag_id \
          WHERE e.type = 'starrocks' AND e.authz_service_id IS NOT NULL \
            AND t.key = $1 AND a.column_name <> '' \
          ORDER BY e.name, e.id, c.name, a.schema_name, a.table_name",
    )
    .bind(tag_key)
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| CompileError::Db(e.to_string()))?;

    let mut out: Vec<ClassifiedEngine> = Vec::new();
    for r in rows {
        let table = ClassifiedTable {
            catalog: r.catalog,
            database: r.schema_name,
            table: r.table_name,
        };
        match out.last_mut() {
            Some(e) if e.engine_id == r.engine_id => e.tables.push(table),
            _ => out.push(ClassifiedEngine {
                engine_id: r.engine_id,
                engine_name: r.engine_name,
                status: r.status,
                origin: r.origin,
                tables: vec![table],
            }),
        }
    }
    Ok(out)
}
