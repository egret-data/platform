//! DB-gated workspace_storages (registry) store tests. Self-skip without DATABASE_URL.
mod common;
use common::{seed_workspace, store};
use egret_catalog::service::storage_conn::StorageConnInput;
use egret_catalog::WorkspaceStorageConn;
use uuid::Uuid;

fn sample_input(name: &str) -> StorageConnInput {
    StorageConnInput {
        name: name.to_string(),
        purpose: Some("data lake".to_string()),
        provider: "s3".to_string(),
        endpoint: Some("http://localhost:9000".to_string()),
        region: "us-east-1".to_string(),
        bucket: "test-bucket".to_string(),
        key_prefix: Some("prefix/".to_string()),
        access_key: "AKIAIOSFODNN7EXAMPLE".to_string(),
        secret_ref: None,
        path_style: true,
    }
}

#[tokio::test]
async fn create_then_list() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    let input = sample_input("primary");

    let conn: WorkspaceStorageConn = s.create_storage_conn(ws, &input).await.unwrap();
    assert_ne!(conn.id, Uuid::nil());
    assert_eq!(conn.workspace_id, ws);
    assert_eq!(conn.name, "primary");
    assert_eq!(conn.bucket, "test-bucket");
    assert!(conn.secret_ref.is_none(), "secret_ref should be None");

    let list = s.list_storage_conns(ws).await.unwrap();
    assert!(
        list.iter().any(|c| c.id == conn.id),
        "created conn must appear in list"
    );
}

#[tokio::test]
async fn get_by_workspace_and_id() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    let other_ws = seed_workspace(&s).await;
    let input = sample_input("primary");

    let conn = s.create_storage_conn(ws, &input).await.unwrap();

    // Correct workspace → found
    let got = s.get_storage_conn(ws, conn.id).await.unwrap();
    assert!(got.is_some(), "should find conn in correct workspace");
    assert_eq!(got.unwrap().id, conn.id);

    // Different workspace → IDOR scope: not found
    let wrong = s.get_storage_conn(other_ws, conn.id).await.unwrap();
    assert!(
        wrong.is_none(),
        "should NOT return conn from a different workspace (IDOR)"
    );
}

#[tokio::test]
async fn update_scoped() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    let other_ws = seed_workspace(&s).await;
    let conn = s
        .create_storage_conn(ws, &sample_input("primary"))
        .await
        .unwrap();

    let updated_input = StorageConnInput {
        bucket: "new-bucket".to_string(),
        ..sample_input("primary")
    };

    // Correct workspace → update succeeds
    let changed = s
        .update_storage_conn(ws, conn.id, &updated_input)
        .await
        .unwrap();
    assert!(changed, "update should return true when row is updated");

    let got = s.get_storage_conn(ws, conn.id).await.unwrap().unwrap();
    assert_eq!(got.bucket, "new-bucket");

    // Different workspace → no-op (scoped update)
    let no_change = s
        .update_storage_conn(other_ws, conn.id, &updated_input)
        .await
        .unwrap();
    assert!(
        !no_change,
        "update from wrong workspace should return false"
    );
}

#[tokio::test]
async fn delete_scoped() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    let other_ws = seed_workspace(&s).await;
    let conn = s
        .create_storage_conn(ws, &sample_input("primary"))
        .await
        .unwrap();

    // Wrong workspace → false, still exists
    let deleted = s.delete_storage_conn(other_ws, conn.id).await.unwrap();
    assert!(!deleted, "delete from wrong workspace should return false");

    let still_there = s.get_storage_conn(ws, conn.id).await.unwrap();
    assert!(still_there.is_some(), "conn should still exist");

    // Correct workspace → true, gone
    let deleted_ok = s.delete_storage_conn(ws, conn.id).await.unwrap();
    assert!(
        deleted_ok,
        "delete from correct workspace should return true"
    );

    let gone = s.get_storage_conn(ws, conn.id).await.unwrap();
    assert!(gone.is_none(), "conn should be gone after delete");

    let list = s.list_storage_conns(ws).await.unwrap();
    assert!(
        !list.iter().any(|c| c.id == conn.id),
        "conn should not appear in list"
    );
}

#[tokio::test]
async fn unique_name_conflict() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    let input = sample_input("dup");

    s.create_storage_conn(ws, &input).await.unwrap();

    // Second create with same name → should error (unique constraint)
    let result = s.create_storage_conn(ws, &input).await;
    assert!(
        result.is_err(),
        "duplicate name in same workspace must fail"
    );
}
