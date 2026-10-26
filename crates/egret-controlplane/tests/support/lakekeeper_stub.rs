//! A minimal stand-in for the embedded catalog's `lakekeeper` tables, for
//! control-plane tests that read a table's state or columns
//! (`egret_perm_compile::table_columns`). A copy of
//! `egret-perm-compile/tests/support/lakekeeper_stub.rs` (a test helper cannot
//! be shared across crates without a crate of its own), plus soft drops.
//!
//! It creates ONLY the tables and columns that module reads, with the same
//! names. Its guard is the drift tests in `egret-catalog-engine`
//! (`tests/table_hook_db.rs`: `the_column_source_reads_what_lakekeeper_loads`,
//! `table_states_follow_lakekeeper`), which run the same queries against real
//! Lakekeeper; if Lakekeeper's tables move, those fail and this stub, and the
//! one it copies, must follow.

// Each test binary that includes this uses a different subset of it.
#![allow(dead_code)]

use egret_metastore::Db;
use serde_json::json;
use uuid::Uuid;

/// Create the stub tables once per database. Serialised by an advisory lock,
/// so tests running in parallel do not race on `CREATE TABLE`.
pub async fn install(db: &Db) {
    let mut tx = db.pool.begin().await.expect("begin");
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('egret-test-lakekeeper-stub'))")
        .execute(&mut *tx)
        .await
        .expect("stub lock");
    for ddl in [
        "CREATE SCHEMA IF NOT EXISTS lakekeeper",
        "CREATE TABLE IF NOT EXISTS lakekeeper.tabular ( \
             tabular_id uuid NOT NULL, warehouse_id uuid NOT NULL, name text NOT NULL, \
             typ text NOT NULL, metadata_location text, deleted_at timestamptz, \
             tabular_namespace_name text[] NOT NULL, PRIMARY KEY (warehouse_id, tabular_id))",
        "CREATE TABLE IF NOT EXISTS lakekeeper.table_schema ( \
             warehouse_id uuid NOT NULL, table_id uuid NOT NULL, schema_id integer NOT NULL, \
             schema jsonb NOT NULL, PRIMARY KEY (warehouse_id, table_id, schema_id))",
        "CREATE TABLE IF NOT EXISTS lakekeeper.table_current_schema ( \
             warehouse_id uuid NOT NULL, table_id uuid NOT NULL, schema_id integer NOT NULL, \
             PRIMARY KEY (warehouse_id, table_id))",
    ] {
        sqlx::query(ddl).execute(&mut *tx).await.expect("stub ddl");
    }
    tx.commit().await.expect("commit");
}

/// Give egret warehouse `warehouse_id` a Lakekeeper warehouse id, returning it.
pub async fn link_warehouse(db: &Db, warehouse_id: Uuid) -> Uuid {
    install(db).await;
    let lk = Uuid::new_v4();
    sqlx::query("UPDATE catalog.warehouses SET lakekeeper_warehouse_id = $2 WHERE id = $1")
        .bind(warehouse_id)
        .bind(lk.to_string())
        .execute(&db.pool)
        .await
        .expect("link warehouse");
    lk
}

/// An active table `namespace.name` whose current schema has `columns` (field
/// ids 1..), returning its tabular id.
pub async fn add_table(
    db: &Db,
    lk: Uuid,
    namespace: &[&str],
    name: &str,
    columns: &[&str],
) -> Uuid {
    let id = add_tabular(
        db,
        lk,
        namespace,
        name,
        "table",
        Some("s3://stub/metadata.json"),
    )
    .await;
    set_columns(db, lk, id, 0, columns).await;
    id
}

/// A view `namespace.name` (no schema rows).
pub async fn add_view(db: &Db, lk: Uuid, namespace: &[&str], name: &str) -> Uuid {
    add_tabular(db, lk, namespace, name, "view", Some("s3://stub/view.json")).await
}

/// A staged create: a table row with no metadata location yet.
pub async fn add_staged_table(db: &Db, lk: Uuid, namespace: &[&str], name: &str, columns: &[&str]) {
    let id = add_tabular(db, lk, namespace, name, "table", None).await;
    set_columns(db, lk, id, 0, columns).await;
}

/// Add schema `schema_id` with `columns` (field ids 1..) and make it current.
pub async fn set_columns(db: &Db, lk: Uuid, table_id: Uuid, schema_id: i32, columns: &[&str]) {
    let fields: Vec<_> = columns
        .iter()
        .enumerate()
        .map(|(i, c)| json!({"id": i + 1, "name": c, "type": "string", "required": false}))
        .collect();
    set_schema(
        db,
        lk,
        table_id,
        schema_id,
        json!({"type": "struct", "schema-id": schema_id, "fields": fields}),
    )
    .await;
}

/// Add schema `schema_id` as the raw Iceberg schema JSON and make it current.
pub async fn set_schema(
    db: &Db,
    lk: Uuid,
    table_id: Uuid,
    schema_id: i32,
    schema: serde_json::Value,
) {
    sqlx::query(
        "INSERT INTO lakekeeper.table_schema (warehouse_id, table_id, schema_id, schema) \
         VALUES ($1,$2,$3,$4)",
    )
    .bind(lk)
    .bind(table_id)
    .bind(schema_id)
    .bind(schema)
    .execute(&db.pool)
    .await
    .expect("stub schema");
    sqlx::query(
        "INSERT INTO lakekeeper.table_current_schema (warehouse_id, table_id, schema_id) \
         VALUES ($1,$2,$3) \
         ON CONFLICT (warehouse_id, table_id) DO UPDATE SET schema_id = EXCLUDED.schema_id",
    )
    .bind(lk)
    .bind(table_id)
    .bind(schema_id)
    .execute(&db.pool)
    .await
    .expect("stub current schema");
}

async fn add_tabular(
    db: &Db,
    lk: Uuid,
    namespace: &[&str],
    name: &str,
    typ: &str,
    metadata_location: Option<&str>,
) -> Uuid {
    let id = Uuid::new_v4();
    let ns: Vec<String> = namespace.iter().map(|s| s.to_string()).collect();
    sqlx::query(
        "INSERT INTO lakekeeper.tabular (tabular_id, warehouse_id, name, typ, \
         metadata_location, tabular_namespace_name) VALUES ($1,$2,$3,$4,$5,$6)",
    )
    .bind(id)
    .bind(lk)
    .bind(name)
    .bind(typ)
    .bind(metadata_location)
    .bind(&ns)
    .execute(&db.pool)
    .await
    .expect("stub tabular");
    id
}

/// Soft-delete tabular `id`, as a drop on a soft-deletion profile does.
pub async fn soft_drop(db: &Db, lk: Uuid, id: Uuid) {
    sqlx::query(
        "UPDATE lakekeeper.tabular SET deleted_at = now() \
          WHERE warehouse_id = $1 AND tabular_id = $2",
    )
    .bind(lk)
    .bind(id)
    .execute(&db.pool)
    .await
    .expect("stub soft drop");
}
