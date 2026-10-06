//! Gated S3 round-trip. Skips unless EGRET_TEST_S3_ENDPOINT is set (e.g. RustFS via `just up`).
use egret_storage::{ObjectStore, S3Store, StorageConfig, StorageError};

fn cfg() -> Option<StorageConfig> {
    let endpoint = std::env::var("EGRET_TEST_S3_ENDPOINT").ok()?;
    Some(StorageConfig {
        endpoint: Some(endpoint),
        region: "us-east-1".into(),
        bucket: std::env::var("EGRET_TEST_S3_BUCKET").unwrap_or_else(|_| "warehouse".into()),
        access_key: std::env::var("EGRET_TEST_S3_KEY").unwrap_or_else(|_| "rustfsadmin".into()),
        secret_key: std::env::var("EGRET_TEST_S3_SECRET").unwrap_or_else(|_| "rustfsadmin".into()),
        path_style: true,
    })
}

#[tokio::test]
async fn put_get_delete_round_trip() {
    let Some(c) = cfg() else {
        eprintln!("EGRET_TEST_S3_ENDPOINT unset — skipping");
        return;
    };
    let bucket = c.bucket.clone();
    let store = S3Store::new(&c).await.unwrap();
    let uri = format!("s3://{bucket}/egret-test/{}.txt", uuid::Uuid::new_v4());
    store.put(&uri, b"hello".to_vec()).await.unwrap();
    assert_eq!(store.get(&uri).await.unwrap(), b"hello");
    store.delete(&uri).await.unwrap();
    assert!(store.get(&uri).await.is_err());
}

#[tokio::test]
async fn copy_object_round_trip() {
    let Some(c) = cfg() else {
        eprintln!("EGRET_TEST_S3_ENDPOINT unset — skipping");
        return;
    };
    let bucket = c.bucket.clone();
    let store = S3Store::new(&c).await.unwrap();
    let dir = format!("egret-test/c-{}", uuid::Uuid::new_v4());
    let src = format!("s3://{bucket}/{dir}/a.txt");
    let dst = format!("s3://{bucket}/{dir}/b.txt");

    store.put(&src, b"copy me".to_vec()).await.unwrap();
    store.copy(&src, &dst).await.unwrap();

    // Both objects exist with identical bytes.
    assert_eq!(store.get(&src).await.unwrap(), b"copy me");
    assert_eq!(store.get(&dst).await.unwrap(), b"copy me");

    // Cleanup.
    store.delete(&src).await.unwrap();
    store.delete(&dst).await.unwrap();
}

/// `CopyObjectError` has no modeled "no such key" variant the way
/// `GetObjectError` does (it only models `ObjectNotInActiveTierError`), so a
/// missing-source `copy_object` against real S3/RustFS falls into
/// `CopyObjectError::Unhandled` rather than a typed variant `copy()` could
/// pattern-match on directly the way `get()` does with `is_no_such_key()`.
/// `egret-apps`'s `restore_version()` fallback (copy the old version's object;
/// on `NotFound`, fall back to a Postgres-column GET+PUT) depends on `copy()`
/// still mapping that case to `StorageError::NotFound`, same as `get()`/
/// `head()` -- this is a live, real-RustFS regression test for that mapping,
/// not just the round-trip `MemStore` already proves for free.
#[tokio::test]
async fn copy_of_a_missing_source_is_not_found() {
    let Some(c) = cfg() else {
        eprintln!("EGRET_TEST_S3_ENDPOINT unset — skipping");
        return;
    };
    let bucket = c.bucket.clone();
    let store = S3Store::new(&c).await.unwrap();
    let dir = format!("egret-test/missing-{}", uuid::Uuid::new_v4());
    let src = format!("s3://{bucket}/{dir}/does-not-exist.txt");
    let dst = format!("s3://{bucket}/{dir}/dst.txt");

    let err = store.copy(&src, &dst).await.unwrap_err();
    assert!(
        matches!(err, StorageError::NotFound(_)),
        "expected NotFound for a missing copy source, got {err:?}"
    );
    // The destination must not have been created by the failed copy.
    assert!(matches!(
        store.get(&dst).await,
        Err(StorageError::NotFound(_))
    ));
}
