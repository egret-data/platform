//! The column source of the StarRocks whole-table clearance (spec §5.2, C2–C4).
//!
//! A classification attached to a whole table compiles one `column: *` deny,
//! which needs no column list, and one NULL mask per column, which does: the
//! base-column mask is what holds through a StarRocks view (A2). egret keeps no
//! column list for tables, so this module reads the table's CURRENT schema from
//! the embedded catalog's own tables (Lakekeeper's `lakekeeper` schema, in the
//! same database), over the compile's own connection: the same snapshot and
//! transaction as the rest of the compile, from every recompile call site.
//!
//! **The coupling is confined here.** Lakekeeper is git-pinned, so these tables
//! change only on a rev bump; the drift test in `egret-catalog-engine`
//! (`tests/table_hook_db.rs`) compares this module's answer with what
//! Lakekeeper itself loads after each schema change, and fails on such a bump.
//! A query error fails the compile (refuse, never guess) rather than skipping
//! masks.
//!
//! "Current schema" (C4): `table_current_schema.schema_id` → `table_schema.schema`
//! (Iceberg schema JSON) → the `name` of each top-level `fields[]` entry, in
//! field-id order. Nested fields are ignored: the mask covers the whole
//! top-level column. Only an active table counts (`typ = 'table'`, not deleted,
//! with a metadata location: Lakekeeper's own "active" predicate), in a
//! single-level namespace (the Ranger resource has one schema level). Anything
//! else is "no schema" (`None`, user decision U4).

use egret_perm::CompileError;
use egret_ranger_eval::resource::fold_identifier;
use serde_json::Value;
use uuid::Uuid;

/// The schema the embedded catalog's tables live in, as the SQL below spells
/// it. The drift test asserts it equals `egret_catalog_engine::LAKEKEEPER_SCHEMA`.
pub const LAKEKEEPER_SCHEMA: &str = "lakekeeper";

/// A table's current top-level columns, every name in the catalog's spelling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableColumns {
    pub schema: String,
    pub table: String,
    pub columns: Vec<String>,
}

/// One table to look up: the egret warehouse (`catalog.warehouses.id`), its
/// schema and its name, in any spelling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableKey {
    pub warehouse_id: Uuid,
    pub schema: String,
    pub table: String,
}

/// The current columns of one table, `None` when it is not an active table of
/// the catalog (unknown, a view, a staged create, dropped, or in a multi-level
/// namespace). The lookup folds case; the answer is the catalog's spelling.
pub async fn current_columns_via(
    conn: &mut sqlx::PgConnection,
    warehouse_id: Uuid,
    schema: &str,
    table: &str,
) -> Result<Option<TableColumns>, CompileError> {
    let key = TableKey {
        warehouse_id,
        schema: schema.to_string(),
        table: table.to_string(),
    };
    Ok(current_columns_batch_via(conn, std::slice::from_ref(&key))
        .await?
        .pop()
        .flatten())
}

/// [`current_columns_via`] for every table of a compile in one query; the
/// answer is in `keys`' order.
pub async fn current_columns_batch_via(
    conn: &mut sqlx::PgConnection,
    keys: &[TableKey],
) -> Result<Vec<Option<TableColumns>>, CompileError> {
    let mut out: Vec<Option<TableColumns>> = vec![None; keys.len()];
    if keys.is_empty() {
        return Ok(out);
    }
    let warehouses: Vec<Uuid> = keys.iter().map(|k| k.warehouse_id).collect();
    let schemas: Vec<String> = keys.iter().map(|k| k.schema.clone()).collect();
    let tables: Vec<String> = keys.iter().map(|k| k.table.clone()).collect();
    // `tabular.name` and `tabular_namespace_name` carry Lakekeeper's
    // `case_insensitive` collation; `lower()` on both sides keeps the fold
    // explicit rather than inherited from it.
    let rows: Vec<(i64, String, String, Value)> = sqlx::query_as(
        "SELECT q.ord, ti.tabular_namespace_name[1], ti.name, ts.schema \
           FROM unnest($1::uuid[], $2::text[], $3::text[]) WITH ORDINALITY \
                AS q(warehouse_id, schema_name, table_name, ord) \
           JOIN catalog.warehouses w ON w.id = q.warehouse_id \
           JOIN lakekeeper.tabular ti ON ti.warehouse_id::text = w.lakekeeper_warehouse_id \
           JOIN lakekeeper.table_current_schema tcs \
             ON tcs.warehouse_id = ti.warehouse_id AND tcs.table_id = ti.tabular_id \
           JOIN lakekeeper.table_schema ts \
             ON ts.warehouse_id = tcs.warehouse_id AND ts.table_id = tcs.table_id \
            AND ts.schema_id = tcs.schema_id \
          WHERE ti.typ = 'table' AND ti.deleted_at IS NULL AND ti.metadata_location IS NOT NULL \
            AND cardinality(ti.tabular_namespace_name) = 1 \
            AND lower(ti.tabular_namespace_name[1]) = lower(q.schema_name) \
            AND lower(ti.name) = lower(q.table_name)",
    )
    .bind(&warehouses)
    .bind(&schemas)
    .bind(&tables)
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| CompileError::Db(format!("reading table columns from the catalog: {e}")))?;
    for (ord, schema, table, iceberg_schema) in rows {
        let i = usize::try_from(ord - 1).expect("ordinality starts at 1");
        out[i] = Some(TableColumns {
            schema,
            table,
            columns: top_level_columns(&iceberg_schema),
        });
    }
    Ok(out)
}

/// The top-level field names of an Iceberg schema, in field-id order. Two
/// names that fold equal (Iceberg allows `Email` beside `email`) are one
/// Ranger resource, so only the lower field id is kept.
fn top_level_columns(schema: &Value) -> Vec<String> {
    let mut fields: Vec<(i64, String)> = schema
        .get("fields")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|f| Some((f.get("id")?.as_i64()?, f.get("name")?.as_str()?.to_string())))
        .collect();
    fields.sort_by_key(|(id, _)| *id);
    let mut out: Vec<String> = Vec::new();
    for (_, name) in fields {
        if !out
            .iter()
            .any(|c| fold_identifier(c) == fold_identifier(&name))
        {
            out.push(name);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn columns_are_the_top_level_fields_in_field_id_order() {
        let schema = json!({"type": "struct", "fields": [
            {"id": 3, "name": "region", "type": "string"},
            {"id": 1, "name": "id", "type": "long"},
            {"id": 2, "name": "address", "type": {"type": "struct", "fields": [
                {"id": 4, "name": "street", "type": "string"}]}},
        ]});
        assert_eq!(top_level_columns(&schema), vec!["id", "address", "region"]);
    }

    #[test]
    fn two_fields_folding_equal_keep_the_lower_field_id() {
        let schema = json!({"type": "struct", "fields": [
            {"id": 7, "name": "email", "type": "string"},
            {"id": 2, "name": "Email", "type": "string"},
        ]});
        assert_eq!(top_level_columns(&schema), vec!["Email"]);
    }
}
