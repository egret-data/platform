//! DB-gated storage_connections (global registry) store tests.
//! Self-skip without DATABASE_URL.
mod common;
use common::store;
use egret_catalog::service::storage_conn::StorageConnInput;
use egret_catalog::{CatalogError, StorageConn};
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

/// Unique name so global (un-scoped) rows never collide across test runs.
fn unique_name(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4().simple())
}

#[tokio::test]
async fn create_list_get_roundtrip() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let name = unique_name("primary");
    let input = sample_input(&name);

    let conn: StorageConn = s.create_storage_conn(&input).await.unwrap();
    assert_ne!(conn.id, Uuid::nil());
    assert_eq!(conn.name, name);
    assert_eq!(conn.bucket, "test-bucket");
    assert!(conn.secret_ref.is_none(), "secret_ref should be None");

    // Global list contains the created row.
    let list = s.list_storage_conns().await.unwrap();
    assert!(
        list.iter().any(|c| c.id == conn.id),
        "created conn must appear in global list"
    );

    // Get by id returns the same row.
    let got = s.get_storage_conn(conn.id).await.unwrap();
    assert!(got.is_some(), "should find conn by id");
    assert_eq!(got.unwrap().id, conn.id);

    // Unknown id → None.
    let missing = s.get_storage_conn(Uuid::new_v4()).await.unwrap();
    assert!(missing.is_none(), "unknown id should return None");
}

#[tokio::test]
async fn update_then_delete() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let name = unique_name("primary");
    let conn = s.create_storage_conn(&sample_input(&name)).await.unwrap();

    let updated_input = StorageConnInput {
        bucket: "new-bucket".to_string(),
        ..sample_input(&name)
    };

    let changed = s.update_storage_conn(conn.id, &updated_input).await.unwrap();
    assert!(changed, "update should return true when a row is updated");

    let got = s.get_storage_conn(conn.id).await.unwrap().unwrap();
    assert_eq!(got.bucket, "new-bucket");

    // Update of a non-existent id → false (no-op).
    let no_change = s
        .update_storage_conn(Uuid::new_v4(), &updated_input)
        .await
        .unwrap();
    assert!(!no_change, "update of unknown id should return false");

    // Delete of a non-existent id → false.
    let deleted_missing = s.delete_storage_conn(Uuid::new_v4()).await.unwrap();
    assert!(!deleted_missing, "delete of unknown id should return false");

    // Delete the real row → true, then gone.
    let deleted_ok = s.delete_storage_conn(conn.id).await.unwrap();
    assert!(deleted_ok, "delete should return true when a row is removed");

    let gone = s.get_storage_conn(conn.id).await.unwrap();
    assert!(gone.is_none(), "conn should be gone after delete");

    let list = s.list_storage_conns().await.unwrap();
    assert!(
        !list.iter().any(|c| c.id == conn.id),
        "conn should not appear in list after delete"
    );
}

#[tokio::test]
async fn duplicate_name_rejected() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let name = unique_name("dup");
    let input = sample_input(&name);

    s.create_storage_conn(&input).await.unwrap();

    // Second create with same (global) name → AlreadyExists.
    let result = s.create_storage_conn(&input).await;
    match result {
        Err(CatalogError::AlreadyExists(n)) => assert_eq!(n, name),
        other => panic!("expected AlreadyExists({name}), got {other:?}"),
    }
}
