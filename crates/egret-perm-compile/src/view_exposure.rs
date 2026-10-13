//! View governance A4: which StarRocks databases hold a classification's
//! columns, per engine, so the control plane can list the materialized views
//! that already exist there.
//!
//! The A3 deny (`crate::clearance::mv_deny_inputs`) stops NEW materialized views
//! in these databases. It cannot remove an existing one, and an existing one may
//! hold a copy of the classified values. This module answers "where could such a
//! copy be" from the same two sources the deny uses: the engine-to-profile
//! binding and `tagged_columns_for_service_via`, grouped by
//! `crate::clearance::database_of`.

use std::collections::BTreeSet;

use sqlx::PgConnection;
use uuid::Uuid;

use egret_perm::CompileError;
use egret_policy::tagged_columns_for_service_via;

use crate::clearance::database_of;
use crate::rules::TaggedColumn;

/// A StarRocks database that holds at least one column of the classification,
/// as reached through one engine.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ClassifiedDatabase {
    pub engine_id: Uuid,
    pub engine_name: String,
    pub catalog: String,
    pub database: String,
}

/// Every `(engine, catalog, database)` on a StarRocks engine bound to a policy
/// profile that holds a column tagged `tag_key`, sorted and without
/// duplicates. Two classified columns in one database are one entry.
pub async fn classified_starrocks_databases(
    conn: &mut PgConnection,
    tag_key: &str,
) -> Result<Vec<ClassifiedDatabase>, CompileError> {
    let engines: Vec<(Uuid, String, i64)> = sqlx::query_as(
        "SELECT id, name, authz_service_id FROM engine_mgmt.engines \
          WHERE type = 'starrocks' AND authz_service_id IS NOT NULL \
          ORDER BY name",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| CompileError::Db(e.to_string()))?;

    let mut out = BTreeSet::new();
    for (engine_id, engine_name, service_id) in engines {
        let columns: Vec<TaggedColumn> =
            tagged_columns_for_service_via(&mut *conn, service_id, tag_key)
                .await
                .map_err(|e| CompileError::Db(e.to_string()))?
                .into_iter()
                .map(|row| TaggedColumn {
                    catalog: row.catalog,
                    schema: row.schema_name,
                    table: row.table_name,
                    column: row.column_name,
                })
                .collect();
        for col in &columns {
            let (catalog, database) = database_of(col);
            out.insert(ClassifiedDatabase {
                engine_id,
                engine_name: engine_name.clone(),
                catalog: catalog.to_string(),
                database: database.to_string(),
            });
        }
    }
    Ok(out.into_iter().collect())
}
