//! D2 (spec §5.2): the column source of a whole-table clearance, over the stub
//! `lakekeeper` tables (`support/lakekeeper_stub.rs`, guarded by the drift test
//! in `egret-catalog-engine`).
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test table_columns

#[path = "support/lakekeeper_stub.rs"]
mod lakekeeper_stub;

use egret_metastore::Db;
use egret_perm_compile::table_columns::{
    current_columns_batch_via, current_columns_via, TableColumns, TableKey,
};
use uuid::Uuid;

async fn test_db() -> Option<Db> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

/// An egret warehouse linked to a fresh stub Lakekeeper warehouse:
/// `(egret warehouse id, lakekeeper warehouse id)`.
async fn warehouse(db: &Db) -> (Uuid, Uuid) {
    let pool = &db.pool;
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,'TC') RETURNING id")
            .bind(unique("tc-ws"))
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-tc-test','AKIATEST') RETURNING id",
    )
    .bind(unique("tc-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let wh: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("tc_wh"))
    .bind(ws)
    .bind(sc)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let lk = lakekeeper_stub::link_warehouse(db, wh).await;
    (wh, lk)
}

async fn columns(db: &Db, wh: Uuid, schema: &str, table: &str) -> Option<TableColumns> {
    let mut conn = db.pool.acquire().await.expect("acquire");
    current_columns_via(&mut conn, wh, schema, table)
        .await
        .expect("column source")
}

/// The lookup folds case, and the answer is the catalog's spelling of the
/// schema, the table and every column, in field-id order.
#[tokio::test]
async fn a_case_folded_lookup_returns_the_catalog_spelling() {
    let Some(db) = test_db().await else { return };
    let (wh, lk) = warehouse(&db).await;
    lakekeeper_stub::add_table(&db, lk, &["Sales"], "Orders", &["Id", "email", "Region"]).await;

    assert_eq!(
        columns(&db, wh, "sales", "ORDERS").await,
        Some(TableColumns {
            schema: "Sales".into(),
            table: "Orders".into(),
            columns: vec!["Id".into(), "email".into(), "Region".into()],
        })
    );
}

/// The CURRENT schema counts, not the first one.
#[tokio::test]
async fn the_current_schema_is_read() {
    let Some(db) = test_db().await else { return };
    let (wh, lk) = warehouse(&db).await;
    let t = lakekeeper_stub::add_table(&db, lk, &["sales"], "orders", &["id"]).await;
    lakekeeper_stub::set_columns(&db, lk, t, 1, &["id", "phone"]).await;

    assert_eq!(
        columns(&db, wh, "sales", "orders").await.map(|c| c.columns),
        Some(vec!["id".to_string(), "phone".to_string()])
    );
}

/// Iceberg allows `Email` beside `email`; Ranger cannot tell them apart, so
/// the lower field id is the one column.
#[tokio::test]
async fn two_fields_folding_equal_collapse_to_the_lower_field_id() {
    let Some(db) = test_db().await else { return };
    let (wh, lk) = warehouse(&db).await;
    let t = lakekeeper_stub::add_table(&db, lk, &["sales"], "orders", &[]).await;
    lakekeeper_stub::set_schema(
        &db,
        lk,
        t,
        1,
        serde_json::json!({"type": "struct", "fields": [
            {"id": 5, "name": "email", "type": "string"},
            {"id": 2, "name": "Email", "type": "string"},
            {"id": 1, "name": "id", "type": "long"},
        ]}),
    )
    .await;

    assert_eq!(
        columns(&db, wh, "sales", "orders").await.map(|c| c.columns),
        Some(vec!["id".to_string(), "Email".to_string()])
    );
}

/// Not an active table in a single-level namespace: no schema (U4).
#[tokio::test]
async fn a_view_a_staged_create_a_multi_level_namespace_and_an_unknown_name_have_no_schema() {
    let Some(db) = test_db().await else { return };
    let (wh, lk) = warehouse(&db).await;
    lakekeeper_stub::add_view(&db, lk, &["sales"], "v_orders").await;
    lakekeeper_stub::add_staged_table(&db, lk, &["sales"], "staged", &["id"]).await;
    lakekeeper_stub::add_table(&db, lk, &["sales", "eu"], "orders", &["id"]).await;
    lakekeeper_stub::add_table(&db, lk, &["sales"], "dropped", &["id"]).await;
    sqlx::query("UPDATE lakekeeper.tabular SET deleted_at = now() WHERE warehouse_id = $1 AND name = 'dropped'")
        .bind(lk)
        .execute(&db.pool)
        .await
        .expect("soft-delete");

    assert_eq!(columns(&db, wh, "sales", "v_orders").await, None);
    assert_eq!(columns(&db, wh, "sales", "staged").await, None);
    assert_eq!(columns(&db, wh, "sales", "orders").await, None);
    assert_eq!(columns(&db, wh, "sales", "dropped").await, None);
    assert_eq!(columns(&db, wh, "sales", "ghost").await, None);
}

/// One query for many tables, answered in the order asked, scoped to each
/// table's own warehouse.
#[tokio::test]
async fn the_batch_answers_in_order_and_per_warehouse() {
    let Some(db) = test_db().await else { return };
    let (wh1, lk1) = warehouse(&db).await;
    let (wh2, lk2) = warehouse(&db).await;
    lakekeeper_stub::add_table(&db, lk1, &["sales"], "orders", &["a"]).await;
    lakekeeper_stub::add_table(&db, lk2, &["sales"], "orders", &["b"]).await;
    let key = |wh: Uuid, t: &str| TableKey {
        warehouse_id: wh,
        schema: "sales".into(),
        table: t.into(),
    };
    let mut conn = db.pool.acquire().await.expect("acquire");
    let got = current_columns_batch_via(
        &mut conn,
        &[key(wh2, "orders"), key(wh1, "ghost"), key(wh1, "orders")],
    )
    .await
    .expect("batch");
    let cols: Vec<Option<Vec<String>>> = got.into_iter().map(|c| c.map(|c| c.columns)).collect();
    assert_eq!(
        cols,
        vec![
            Some(vec!["b".to_string()]),
            None,
            Some(vec!["a".to_string()])
        ]
    );
}
