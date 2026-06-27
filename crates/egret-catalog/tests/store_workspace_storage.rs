//! DB-gated workspace_storage store tests. Self-skip without DATABASE_URL.
mod common;
use common::{seed_workspace, store};
use egret_catalog::WorkspaceStorageRow;
use uuid::Uuid;

#[tokio::test]
async fn upsert_then_get_round_trips() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    let secret = Uuid::new_v4();
    let row = WorkspaceStorageRow {
        workspace_id: ws,
        provider: "s3".to_string(),
        endpoint: Some("http://localhost:9000".to_string()),
        region: "us-east-1".to_string(),
        bucket: "wh".to_string(),
        key_prefix: None,
        access_key: "ak".to_string(),
        secret_ref: Some(secret),
        path_style: true,
    };
    s.upsert_workspace_storage(&row).await.unwrap();

    let got = s.workspace_storage_for(ws).await.unwrap();
    assert!(got.is_some(), "expected Some after upsert");
    assert_eq!(got.unwrap(), row);
}

#[tokio::test]
async fn upsert_is_idempotent_update() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    let row1 = WorkspaceStorageRow {
        workspace_id: ws,
        provider: "s3".to_string(),
        endpoint: None,
        region: "us-east-1".to_string(),
        bucket: "bucket-first".to_string(),
        key_prefix: None,
        access_key: "ak".to_string(),
        secret_ref: None,
        path_style: true,
    };
    s.upsert_workspace_storage(&row1).await.unwrap();

    let row2 = WorkspaceStorageRow {
        bucket: "bucket-second".to_string(),
        ..row1.clone()
    };
    s.upsert_workspace_storage(&row2).await.unwrap();

    let got = s.workspace_storage_for(ws).await.unwrap().unwrap();
    assert_eq!(
        got.bucket, "bucket-second",
        "upsert must update existing row"
    );
    // Still only one row for this workspace.
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workspace_storage WHERE workspace_id = $1")
            .bind(ws)
            .fetch_one(s.pool())
            .await
            .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn get_absent_returns_none() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let got = s.workspace_storage_for(Uuid::new_v4()).await.unwrap();
    assert!(got.is_none());
}
