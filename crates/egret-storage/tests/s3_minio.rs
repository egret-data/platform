//! Gated S3 round-trip. Skips unless EGRET_TEST_S3_ENDPOINT is set (e.g. MinIO via `just up`).
use egret_storage::{ObjectStore, S3Store, StorageConfig};

fn cfg() -> Option<StorageConfig> {
    let endpoint = std::env::var("EGRET_TEST_S3_ENDPOINT").ok()?;
    Some(StorageConfig {
        endpoint: Some(endpoint),
        region: "us-east-1".into(),
        bucket: std::env::var("EGRET_TEST_S3_BUCKET").unwrap_or_else(|_| "warehouse".into()),
        access_key: std::env::var("EGRET_TEST_S3_KEY").unwrap_or_else(|_| "minioadmin".into()),
        secret_key: std::env::var("EGRET_TEST_S3_SECRET").unwrap_or_else(|_| "minioadmin".into()),
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
    let store = S3Store::new(&c).await;
    let uri = format!("s3://{bucket}/egret-test/{}.txt", uuid::Uuid::new_v4());
    store.put(&uri, b"hello".to_vec()).await.unwrap();
    assert_eq!(store.get(&uri).await.unwrap(), b"hello");
    store.delete(&uri).await.unwrap();
    assert!(store.get(&uri).await.is_err());
}
