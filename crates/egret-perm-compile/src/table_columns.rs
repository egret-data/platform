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
//!
//! **The overlay (C11).** The table-commit hook dry-runs the recompile a
//! schema-growing Iceberg commit would cause, BEFORE the commit, in a
//! transaction it rolls back. It sets [`PENDING_COLUMNS_SETTING`] `LOCAL` to
//! the columns the commit would leave each table with; inside that
//! transaction those answer for their tables instead of the stored schema. A
//! `LOCAL` setting dies with its transaction, so no other compile sees it.
//!
//! **Leaving coordinates (rename spec C8).** While the hook holds a rename,
//! the source table (or a renamed column's old name) still exists and must
//! stay protected, so both names compile. An overlay entry marks the source
//! `leaving_table`, or lists the old column names in `leaving`; the expansion
//! ceilings skip policies on those coordinates ([`Leaving`]), so a move is
//! counted once. The policies are still emitted.

use std::collections::HashSet;

use egret_perm::CompileError;
use egret_ranger_eval::resource::fold_identifier;
use serde_json::Value;
use uuid::Uuid;

/// The schema the embedded catalog's tables live in, as the SQL below spells
/// it. The drift test asserts it equals `egret_catalog_engine::LAKEKEEPER_SCHEMA`.
pub const LAKEKEEPER_SCHEMA: &str = "lakekeeper";

/// The transaction-local setting the overlay is read from: a JSON array of
/// [`PendingColumns`].
pub const PENDING_COLUMNS_SETTING: &str = "egret.pending_table_columns";

/// One table's columns as a pending commit would leave them; `columns: None`
/// when the commit leaves no table under that name (a rename's source).
/// `leaving_table` and `leaving` mark what a held move is leaving (module
/// doc); both default to nothing, so an overlay without them still parses.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PendingColumns {
    pub warehouse_id: Uuid,
    pub schema: String,
    pub table: String,
    pub columns: Option<Vec<String>>,
    #[serde(default)]
    pub leaving: Vec<String>,
    #[serde(default)]
    pub leaving_table: bool,
}

/// The coordinates the current transaction's overlay marks as leaving, by
/// folded catalog name (a compiled policy names its catalog, not its
/// warehouse; `registry.catalogs.name` is globally unique).
#[derive(Debug, Default)]
pub struct Leaving {
    tables: HashSet<(String, String, String)>,
    columns: HashSet<(String, String, String, String)>,
}

impl Leaving {
    /// Does a policy on `catalog.schema.table` (and `column`, when it names
    /// one) count toward an expansion ceiling? Not when its table or its
    /// column is leaving.
    pub fn counts_toward_ceiling(
        &self,
        catalog: &str,
        schema: &str,
        table: &str,
        column: Option<&str>,
    ) -> bool {
        if self.tables.is_empty() && self.columns.is_empty() {
            return true;
        }
        let t = (
            fold_identifier(catalog),
            fold_identifier(schema),
            fold_identifier(table),
        );
        if self.tables.contains(&t) {
            return false;
        }
        match column {
            Some(c) => !self.columns.contains(&(t.0, t.1, t.2, fold_identifier(c))),
            None => true,
        }
    }

    /// [`Self::counts_toward_ceiling`] for a compiled body, read from every
    /// value at each of `levels` (catalog, schema, table, column): the body
    /// is uncounted only when every coordinate it names is leaving (review
    /// finding 9). A body that names no table (a tag policy) always counts.
    pub fn counts_body(&self, body: &Value, levels: &[&str]) -> bool {
        let values = |level: &str| -> Vec<&str> {
            body.get("resources")
                .and_then(|r| r.get(level))
                .and_then(|l| l.get("values"))
                .and_then(Value::as_array)
                .map(|vs| vs.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default()
        };
        let [catalog, schema, table, column, ..] = levels else {
            return true;
        };
        let (catalogs, schemas, tables) = (values(catalog), values(schema), values(table));
        if catalogs.is_empty() || schemas.is_empty() || tables.is_empty() {
            return true;
        }
        let columns: Vec<Option<&str>> = match values(column) {
            cs if cs.is_empty() => vec![None],
            cs => cs.into_iter().map(Some).collect(),
        };
        catalogs.iter().any(|c| {
            schemas.iter().any(|s| {
                tables.iter().any(|t| {
                    columns
                        .iter()
                        .any(|col| self.counts_toward_ceiling(c, s, t, *col))
                })
            })
        })
    }
}

/// The leaving coordinates of the current transaction's overlay (module doc),
/// empty outside a held move.
pub async fn leaving_via(conn: &mut sqlx::PgConnection) -> Result<Leaving, CompileError> {
    let pending: Vec<PendingColumns> = pending_columns(conn)
        .await?
        .into_iter()
        .filter(|p| p.leaving_table || !p.leaving.is_empty())
        .collect();
    let mut out = Leaving::default();
    if pending.is_empty() {
        return Ok(out);
    }
    let warehouses: Vec<Uuid> = pending.iter().map(|p| p.warehouse_id).collect();
    let catalogs: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT warehouse_id, name FROM registry.catalogs WHERE warehouse_id = ANY($1)",
    )
    .bind(&warehouses)
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| CompileError::Db(format!("reading the catalogs of a held move: {e}")))?;
    for p in &pending {
        for (_, catalog) in catalogs.iter().filter(|(w, _)| *w == p.warehouse_id) {
            let t = (
                fold_identifier(catalog),
                fold_identifier(&p.schema),
                fold_identifier(&p.table),
            );
            for c in &p.leaving {
                out.columns
                    .insert((t.0.clone(), t.1.clone(), t.2.clone(), fold_identifier(c)));
            }
            if p.leaving_table {
                out.tables.insert(t);
            }
        }
    }
    Ok(out)
}

/// A table's current top-level fields, `(field id, name)` in field-id order,
/// every name in the catalog's spelling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableFields {
    pub schema: String,
    pub table: String,
    pub fields: Vec<(i64, String)>,
}

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
    let pending = pending_columns(conn).await?;
    let overlaid = |k: &TableKey| {
        pending.iter().find(|p| {
            p.warehouse_id == k.warehouse_id
                && fold_identifier(&p.schema) == fold_identifier(&k.schema)
                && fold_identifier(&p.table) == fold_identifier(&k.table)
        })
    };
    let rows = stored_current_schemas(conn, keys).await?;
    for (ord, schema, table, iceberg_schema) in rows {
        let i = usize::try_from(ord - 1).expect("ordinality starts at 1");
        out[i] = Some(TableColumns {
            schema,
            table,
            columns: top_level_columns(&iceberg_schema),
        });
    }
    for (i, k) in keys.iter().enumerate() {
        if let Some(p) = overlaid(k) {
            out[i] = p.columns.as_ref().map(|columns| TableColumns {
                schema: p.schema.clone(),
                table: p.table.clone(),
                columns: columns.clone(),
            });
        }
    }
    Ok(out)
}

/// The stored current schema of each of `keys` that is an active table, as
/// `(1-based position in keys, schema, table, Iceberg schema JSON)`.
async fn stored_current_schemas(
    conn: &mut sqlx::PgConnection,
    keys: &[TableKey],
) -> Result<Vec<(i64, String, String, Value)>, CompileError> {
    let warehouses: Vec<Uuid> = keys.iter().map(|k| k.warehouse_id).collect();
    let schemas: Vec<String> = keys.iter().map(|k| k.schema.clone()).collect();
    let tables: Vec<String> = keys.iter().map(|k| k.table.clone()).collect();
    // `tabular.name` and `tabular_namespace_name` carry Lakekeeper's
    // `case_insensitive` collation; `lower()` on both sides keeps the fold
    // explicit rather than inherited from it.
    sqlx::query_as(
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
    .map_err(|e| CompileError::Db(format!("reading table columns from the catalog: {e}")))
}

/// The STORED current top-level fields of one table, `None` when it is not an
/// active table of the catalog (as [`current_columns_via`]). The overlay is
/// not applied: this is what the catalog holds now, which the table hook
/// compares a commit's new schema with.
pub async fn current_fields_via(
    conn: &mut sqlx::PgConnection,
    warehouse_id: Uuid,
    schema: &str,
    table: &str,
) -> Result<Option<TableFields>, CompileError> {
    let key = TableKey {
        warehouse_id,
        schema: schema.to_string(),
        table: table.to_string(),
    };
    Ok(stored_current_schemas(conn, std::slice::from_ref(&key))
        .await?
        .pop()
        .map(|(_, schema, table, iceberg_schema)| TableFields {
            schema,
            table,
            fields: top_level_fields(&iceberg_schema),
        }))
}

/// What the catalog holds under a classified table's name (rename spec C14):
/// a live table or view (`Live`; a view's rows are the view hold's), only
/// soft-deleted tabulars (`Dropped`, an undrop brings one back), or nothing
/// (`Missing`: renamed away, purged, or classified before it was created,
/// which cannot be told apart).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum TableState {
    Live,
    Dropped,
    Missing,
}

/// The [`TableState`] of each of `keys`, in `keys`' order, in one query. A
/// multi-level namespace holds no classification row and is `Missing`; so is
/// a warehouse with no Lakekeeper warehouse.
pub async fn table_states_via(
    conn: &mut sqlx::PgConnection,
    keys: &[TableKey],
) -> Result<Vec<TableState>, CompileError> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let warehouses: Vec<Uuid> = keys.iter().map(|k| k.warehouse_id).collect();
    let schemas: Vec<String> = keys.iter().map(|k| k.schema.clone()).collect();
    let tables: Vec<String> = keys.iter().map(|k| k.table.clone()).collect();
    let rows: Vec<(i64, bool, bool)> = sqlx::query_as(
        "SELECT q.ord, \
                COALESCE(bool_or(t.deleted_at IS NULL AND t.metadata_location IS NOT NULL), \
                         false), \
                COALESCE(bool_or(t.deleted_at IS NOT NULL), false) \
           FROM unnest($1::uuid[], $2::text[], $3::text[]) WITH ORDINALITY \
                AS q(warehouse_id, schema_name, table_name, ord) \
           LEFT JOIN catalog.warehouses w ON w.id = q.warehouse_id \
           LEFT JOIN lakekeeper.tabular t \
             ON t.warehouse_id::text = w.lakekeeper_warehouse_id \
            AND cardinality(t.tabular_namespace_name) = 1 \
            AND lower(t.tabular_namespace_name[1]) = lower(q.schema_name) \
            AND lower(t.name) = lower(q.table_name) \
          GROUP BY q.ord ORDER BY q.ord",
    )
    .bind(&warehouses)
    .bind(&schemas)
    .bind(&tables)
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| CompileError::Db(format!("reading table states from the catalog: {e}")))?;
    Ok(rows
        .into_iter()
        .map(|(_, live, dropped)| {
            if live {
                TableState::Live
            } else if dropped {
                TableState::Dropped
            } else {
                TableState::Missing
            }
        })
        .collect())
}

/// The Lakekeeper `tabular_id` (the Iceberg table uuid, which a rename keeps)
/// of the active table `schema.table`, `None` when there is none. The table
/// hook compares it before and after a rename Lakekeeper answered non-2xx, to
/// tell whether the rename committed anyway (review finding 4).
pub async fn live_tabular_id_via(
    conn: &mut sqlx::PgConnection,
    warehouse_id: Uuid,
    schema: &str,
    table: &str,
) -> Result<Option<Uuid>, CompileError> {
    sqlx::query_scalar(
        "SELECT ti.tabular_id FROM catalog.warehouses w \
           JOIN lakekeeper.tabular ti ON ti.warehouse_id::text = w.lakekeeper_warehouse_id \
          WHERE w.id = $1 AND ti.typ = 'table' AND ti.deleted_at IS NULL \
            AND ti.metadata_location IS NOT NULL \
            AND cardinality(ti.tabular_namespace_name) = 1 \
            AND lower(ti.tabular_namespace_name[1]) = lower($2) AND lower(ti.name) = lower($3)",
    )
    .bind(warehouse_id)
    .bind(schema)
    .bind(table)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| CompileError::Db(format!("reading a table id from the catalog: {e}")))
}

/// Whether each of `keys` names a SOFT-DELETED tabular (any number may share
/// a name beside one live one, rename spec E4). Rows are keyed by name, so a
/// dropped twin still needs the name's rows: an undrop restores it under that
/// name (review finding 1). In `keys`' order, in one query.
pub async fn has_dropped_tabular_via(
    conn: &mut sqlx::PgConnection,
    keys: &[TableKey],
) -> Result<Vec<bool>, CompileError> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let warehouses: Vec<Uuid> = keys.iter().map(|k| k.warehouse_id).collect();
    let schemas: Vec<String> = keys.iter().map(|k| k.schema.clone()).collect();
    let tables: Vec<String> = keys.iter().map(|k| k.table.clone()).collect();
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM catalog.warehouses w \
                          JOIN lakekeeper.tabular t \
                            ON t.warehouse_id::text = w.lakekeeper_warehouse_id \
                         WHERE w.id = q.warehouse_id AND t.deleted_at IS NOT NULL \
                           AND cardinality(t.tabular_namespace_name) = 1 \
                           AND lower(t.tabular_namespace_name[1]) = lower(q.schema_name) \
                           AND lower(t.name) = lower(q.table_name)) \
           FROM unnest($1::uuid[], $2::text[], $3::text[]) WITH ORDINALITY \
                AS q(warehouse_id, schema_name, table_name, ord) \
          ORDER BY q.ord",
    )
    .bind(&warehouses)
    .bind(&schemas)
    .bind(&tables)
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| CompileError::Db(format!("reading dropped tables from the catalog: {e}")))
}

/// A classified column a live table does not have (rename spec R8, §5.8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingColumn {
    pub key: TableKey,
    /// The column as the classification row spells it.
    pub column: String,
    /// The current name of the field that last carried this name in an older
    /// schema of the table, when that field is still there under another
    /// name. A candidate for a steward to confirm, never applied: a name
    /// dropped and later re-added for a new field resolves to the newest one.
    pub renamed_to: Option<String>,
}

/// Which of each table's `columns` its current schema lacks (folded), with
/// the rename candidate from the schema history (rename spec §5.8): the field
/// id that carried the name in the most recent older schema that had it
/// (highest `schema_id` below the current one). Only active tables answer;
/// any other key reports nothing. One query.
pub async fn missing_columns_via(
    conn: &mut sqlx::PgConnection,
    tables: &[(TableKey, Vec<String>)],
) -> Result<Vec<MissingColumn>, CompileError> {
    if tables.is_empty() {
        return Ok(Vec::new());
    }
    let warehouses: Vec<Uuid> = tables.iter().map(|(k, _)| k.warehouse_id).collect();
    let schemas: Vec<String> = tables.iter().map(|(k, _)| k.schema.clone()).collect();
    let names: Vec<String> = tables.iter().map(|(k, _)| k.table.clone()).collect();
    let rows: Vec<(i64, i32, i32, Value)> = sqlx::query_as(
        "SELECT q.ord, tcs.schema_id, ts.schema_id, ts.schema \
           FROM unnest($1::uuid[], $2::text[], $3::text[]) WITH ORDINALITY \
                AS q(warehouse_id, schema_name, table_name, ord) \
           JOIN catalog.warehouses w ON w.id = q.warehouse_id \
           JOIN lakekeeper.tabular ti ON ti.warehouse_id::text = w.lakekeeper_warehouse_id \
           JOIN lakekeeper.table_current_schema tcs \
             ON tcs.warehouse_id = ti.warehouse_id AND tcs.table_id = ti.tabular_id \
           JOIN lakekeeper.table_schema ts \
             ON ts.warehouse_id = ti.warehouse_id AND ts.table_id = ti.tabular_id \
          WHERE ti.typ = 'table' AND ti.deleted_at IS NULL AND ti.metadata_location IS NOT NULL \
            AND cardinality(ti.tabular_namespace_name) = 1 \
            AND lower(ti.tabular_namespace_name[1]) = lower(q.schema_name) \
            AND lower(ti.name) = lower(q.table_name) \
          ORDER BY q.ord, ts.schema_id DESC",
    )
    .bind(&warehouses)
    .bind(&schemas)
    .bind(&names)
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| CompileError::Db(format!("reading table schemas from the catalog: {e}")))?;
    let mut out = Vec::new();
    for (i, (key, columns)) in tables.iter().enumerate() {
        let ord = i as i64 + 1;
        // Newest first: the current schema, then older ones by falling id.
        let mut current: Option<Vec<(i64, String)>> = None;
        let mut older: Vec<Vec<(i64, String)>> = Vec::new();
        for (_, current_id, schema_id, schema) in rows.iter().filter(|r| r.0 == ord) {
            if schema_id == current_id {
                current = Some(top_level_fields(schema));
            } else if schema_id < current_id {
                older.push(top_level_fields(schema));
            }
        }
        let Some(current) = current else {
            continue;
        };
        let folded = |n: &str| fold_identifier(n);
        for column in columns {
            if current.iter().any(|(_, n)| folded(n) == folded(column)) {
                continue;
            }
            let renamed_to = older
                .iter()
                .find_map(|fields| fields.iter().find(|(_, n)| folded(n) == folded(column)))
                .and_then(|(id, _)| current.iter().find(|(i, _)| i == id))
                .map(|(_, n)| n.clone());
            out.push(MissingColumn {
                key: key.clone(),
                column: column.clone(),
                renamed_to,
            });
        }
    }
    Ok(out)
}

/// The columns a SOFT-DELETED table would come back with if it were undropped:
/// the catalog's spelling of its single-level schema and name, and its current
/// columns, by its Lakekeeper `tabular_id`. `None` for anything else (a view, a
/// live table, a multi-level namespace, an unknown id).
pub async fn dropped_table_columns_via(
    conn: &mut sqlx::PgConnection,
    warehouse_id: Uuid,
    tabular_id: Uuid,
) -> Result<Option<TableColumns>, CompileError> {
    let row: Option<(String, String, Value)> = sqlx::query_as(
        "SELECT ti.tabular_namespace_name[1], ti.name, ts.schema \
           FROM catalog.warehouses w \
           JOIN lakekeeper.tabular ti ON ti.warehouse_id::text = w.lakekeeper_warehouse_id \
           JOIN lakekeeper.table_current_schema tcs \
             ON tcs.warehouse_id = ti.warehouse_id AND tcs.table_id = ti.tabular_id \
           JOIN lakekeeper.table_schema ts \
             ON ts.warehouse_id = tcs.warehouse_id AND ts.table_id = tcs.table_id \
            AND ts.schema_id = tcs.schema_id \
          WHERE w.id = $1 AND ti.tabular_id = $2 AND ti.typ = 'table' \
            AND ti.deleted_at IS NOT NULL AND cardinality(ti.tabular_namespace_name) = 1",
    )
    .bind(warehouse_id)
    .bind(tabular_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| CompileError::Db(format!("reading a dropped table's columns: {e}")))?;
    Ok(row.map(|(schema, table, iceberg_schema)| TableColumns {
        schema,
        table,
        columns: top_level_columns(&iceberg_schema),
    }))
}

/// The overlay of the current transaction, empty outside a dry run. A custom
/// setting that was set once in a session reads `''` after its transaction,
/// which is no overlay.
async fn pending_columns(
    conn: &mut sqlx::PgConnection,
) -> Result<Vec<PendingColumns>, CompileError> {
    let raw: Option<String> = sqlx::query_scalar("SELECT current_setting($1, true)")
        .bind(PENDING_COLUMNS_SETTING)
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| CompileError::Db(format!("reading the pending table columns: {e}")))?;
    match raw.as_deref() {
        None | Some("") => Ok(Vec::new()),
        Some(json) => serde_json::from_str(json).map_err(|e| {
            CompileError::Db(format!("the pending table columns are unreadable: {e}"))
        }),
    }
}

/// The columns of the STORED schema `schema_id` of a table (a commit that sets
/// an older schema current), `None` when the table or the schema is unknown.
pub async fn stored_schema_columns_via(
    conn: &mut sqlx::PgConnection,
    warehouse_id: Uuid,
    schema: &str,
    table: &str,
    schema_id: i64,
) -> Result<Option<Vec<String>>, CompileError> {
    Ok(
        stored_schema_fields_via(conn, warehouse_id, schema, table, schema_id)
            .await?
            .map(|fields| column_names(&fields)),
    )
}

/// [`stored_schema_columns_via`] as `(field id, name)` pairs.
pub async fn stored_schema_fields_via(
    conn: &mut sqlx::PgConnection,
    warehouse_id: Uuid,
    schema: &str,
    table: &str,
    schema_id: i64,
) -> Result<Option<Vec<(i64, String)>>, CompileError> {
    let found: Option<Value> = sqlx::query_scalar(
        "SELECT ts.schema \
           FROM catalog.warehouses w \
           JOIN lakekeeper.tabular ti ON ti.warehouse_id::text = w.lakekeeper_warehouse_id \
           JOIN lakekeeper.table_schema ts \
             ON ts.warehouse_id = ti.warehouse_id AND ts.table_id = ti.tabular_id \
          WHERE w.id = $1 AND ti.typ = 'table' AND ti.deleted_at IS NULL \
            AND ti.metadata_location IS NOT NULL \
            AND cardinality(ti.tabular_namespace_name) = 1 \
            AND lower(ti.tabular_namespace_name[1]) = lower($2) \
            AND lower(ti.name) = lower($3) AND ts.schema_id = $4",
    )
    .bind(warehouse_id)
    .bind(schema)
    .bind(table)
    .bind(i32::try_from(schema_id).unwrap_or(-1))
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| CompileError::Db(format!("reading a stored table schema: {e}")))?;
    Ok(found.as_ref().map(top_level_fields))
}

/// The top-level fields of an Iceberg schema, `(field id, name)` in field-id
/// order, every field kept (two names folding equal included). Public: the
/// table hook pairs a column rename by field id with it.
pub fn top_level_fields(schema: &Value) -> Vec<(i64, String)> {
    let mut fields: Vec<(i64, String)> = schema
        .get("fields")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|f| Some((f.get("id")?.as_i64()?, f.get("name")?.as_str()?.to_string())))
        .collect();
    fields.sort_by_key(|(id, _)| *id);
    fields
}

/// The top-level field names of an Iceberg schema, in field-id order. Two
/// names that fold equal (Iceberg allows `Email` beside `email`) are one
/// Ranger resource, so only the lower field id is kept. Public: the table hook
/// reads a pending commit's columns from its request with it.
pub fn top_level_columns(schema: &Value) -> Vec<String> {
    column_names(&top_level_fields(schema))
}

/// The column names of `fields` (in field-id order), two names that fold
/// equal collapsed to the first (as [`top_level_columns`]).
pub fn column_names(fields: &[(i64, String)]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (_, name) in fields {
        if !out
            .iter()
            .any(|c| fold_identifier(c) == fold_identifier(name))
        {
            out.push(name.clone());
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

    #[test]
    fn top_level_fields_keeps_ids_in_field_id_order() {
        let schema = json!({"type": "struct", "fields": [
            {"id": 7, "name": "email", "type": "string"},
            {"id": 3, "name": "region", "type": "string"},
            {"id": 2, "name": "Email", "type": "string"},
        ]});
        assert_eq!(
            top_level_fields(&schema),
            vec![
                (2, "Email".to_string()),
                (3, "region".to_string()),
                (7, "email".to_string())
            ]
        );
        assert_eq!(top_level_columns(&schema), vec!["Email", "region"]);
    }

    #[test]
    fn a_body_counts_unless_every_coordinate_it_names_is_leaving() {
        let mut leaving = Leaving::default();
        leaving
            .tables
            .insert(("lake".into(), "sales".into(), "orders".into()));
        let levels = ["catalog", "database", "table", "column"];
        let body = |tables: &[&str]| {
            json!({"resources": {
                "catalog": {"values": ["lake"]}, "database": {"values": ["sales"]},
                "table": {"values": tables}, "column": {"values": ["*"]}}})
        };
        assert!(!leaving.counts_body(&body(&["orders"]), &levels));
        assert!(!leaving.counts_body(&body(&["Orders"]), &levels));
        assert!(leaving.counts_body(&body(&["orders", "events"]), &levels));
        assert!(leaving.counts_body(&body(&["events", "orders"]), &levels));
        assert!(leaving.counts_body(&json!({"resources": {"tag": {"values": ["pii"]}}}), &levels));
    }

    #[test]
    fn an_overlay_without_leaving_still_parses() {
        let raw = r#"[{"warehouse_id": "00000000-0000-0000-0000-000000000001",
                       "schema": "sales", "table": "orders", "columns": ["id"]}]"#;
        let parsed: Vec<PendingColumns> = serde_json::from_str(raw).expect("a #755 overlay");
        assert!(parsed[0].leaving.is_empty());
        assert!(!parsed[0].leaving_table);
    }
}
