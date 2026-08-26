//! DB-gated QueryStore tests. Self-skip without DATABASE_URL.
use egret_core::{ColumnMeta, TabularResult};
use egret_metastore::Db;
use egret_query::execution::model::QueryStatus;
use egret_query::QueryStore;
use uuid::Uuid;

async fn setup() -> Option<(QueryStore, Uuid, Uuid, Uuid)> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let uid: Uuid =
        sqlx::query_scalar("INSERT INTO users (email, provider) VALUES ($1,'local') RETURNING id")
            .bind(format!("u_{}@t.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let wid: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by) VALUES ($1,'t',$2) RETURNING id",
    )
    .bind(format!("w-{}", Uuid::new_v4().simple()))
    .bind(uid)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let did: Uuid = sqlx::query_scalar("INSERT INTO query.databases (name, engine, host, port, workspace_id) VALUES ($1,'trino','h',8080,$2) RETURNING id")
        .bind(format!("db_{}", Uuid::new_v4().simple())).bind(wid).fetch_one(&db.pool).await.unwrap();
    Some((QueryStore::new(db), wid, did, uid))
}

#[tokio::test]
async fn lifecycle_queued_running_finished() {
    let Some((store, wid, did, uid)) = setup().await else {
        eprintln!("skip");
        return;
    };
    let id = store.insert(wid, did, Some(uid), "SELECT 1").await.unwrap();
    assert_eq!(
        store.get(id).await.unwrap().unwrap().status,
        QueryStatus::Queued
    );
    store.set_running(id).await.unwrap();
    assert_eq!(
        store.get(id).await.unwrap().unwrap().status,
        QueryStatus::Running
    );
    let result = TabularResult {
        columns: vec![ColumnMeta {
            name: "n".into(),
            ty: "bigint".into(),
        }],
        rows: vec![vec![serde_json::json!(1)], vec![serde_json::json!(2)]],
        row_count: 2,
        bytes_scanned: Some(10),
        truncated: false,
    };
    store
        .finish(id, &result, "s3://warehouse/query-results/x.jsonl", 1)
        .await
        .unwrap();
    let q = store.get(id).await.unwrap().unwrap();
    assert_eq!(q.status, QueryStatus::Finished);
    assert_eq!(q.row_count, Some(2));
    assert!(q.result_ref.is_some());
    // preview is capped to 1 row
    assert_eq!(q.preview.unwrap().as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn fail_and_cancel_set_status() {
    let Some((store, wid, did, _uid)) = setup().await else {
        eprintln!("skip");
        return;
    };
    let id = store.insert(wid, did, None, "SELECT 1").await.unwrap();
    store.fail(id, "boom").await.unwrap();
    let q = store.get(id).await.unwrap().unwrap();
    assert_eq!(q.status, QueryStatus::Failed);
    assert_eq!(q.error.as_deref(), Some("boom"));
    let id2 = store.insert(wid, did, None, "SELECT 1").await.unwrap();
    store.set_cancelled(id2).await.unwrap();
    assert_eq!(
        store.get(id2).await.unwrap().unwrap().status,
        QueryStatus::Cancelled
    );
}

#[tokio::test]
async fn jsonl_serialization_roundtrip() {
    let result = TabularResult {
        columns: vec![ColumnMeta {
            name: "n".into(),
            ty: "bigint".into(),
        }],
        rows: vec![vec![serde_json::json!(1)], vec![serde_json::json!("x")]],
        row_count: 2,
        bytes_scanned: None,
        truncated: false,
    };
    let bytes = QueryStore::result_to_jsonl(&result);
    let page = QueryStore::jsonl_slice(&bytes, 0, 10);
    assert_eq!(page.len(), 2);
    assert_eq!(page[0], serde_json::json!([1]));
    assert_eq!(page[1], serde_json::json!(["x"]));
    // pagination offset
    let p2 = QueryStore::jsonl_slice(&bytes, 1, 10);
    assert_eq!(p2.len(), 1);
    assert_eq!(p2[0], serde_json::json!(["x"]));
}
