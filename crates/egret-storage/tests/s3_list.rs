//! Folder-style S3 listing test. Gated on EGRET_TEST_S3_ENDPOINT (RustFS via `just up`).
use egret_storage::{ObjectStore, S3Store, StorageConfig};

fn cfg() -> Option<StorageConfig> {
    let endpoint = std::env::var("EGRET_TEST_S3_ENDPOINT").ok()?;
    Some(StorageConfig {
        endpoint: Some(endpoint),
        region: "us-east-1".into(),
        bucket: std::env::var("EGRET_TEST_S3_BUCKET").unwrap_or_else(|_| "warehouse".into()),
        access_key: std::env::var("EGRET_TEST_S3_KEY").unwrap_or_else(|_| "egret".into()),
        secret_key: std::env::var("EGRET_TEST_S3_SECRET").unwrap_or_else(|_| "egretsecret".into()),
        path_style: true,
    })
}

#[tokio::test]
async fn list_folder_style_and_flat() {
    let Some(c) = cfg() else {
        eprintln!("EGRET_TEST_S3_ENDPOINT unset — skipping s3_list tests");
        return;
    };
    let bucket = c.bucket.clone();
    let store = S3Store::new(&c).await.unwrap();

    // Unique root prefix so parallel test runs don't collide.
    let run_id = uuid::Uuid::new_v4();
    let root = format!("egret-list-test/{run_id}");

    let uri_a = format!("s3://{bucket}/{root}/a.txt");
    let uri_b = format!("s3://{bucket}/{root}/b.txt");
    let uri_c = format!("s3://{bucket}/{root}/sub/c.txt");

    // ── arrange ──────────────────────────────────────────────────────────────
    store.put(&uri_a, b"file-a".to_vec()).await.unwrap();
    store.put(&uri_b, b"file-b".to_vec()).await.unwrap();
    store.put(&uri_c, b"file-c".to_vec()).await.unwrap();

    let prefix = format!("{root}/");

    // ── act: folder-style (delimiter="/") ────────────────────────────────────
    let folder_listing = store.list(&prefix, Some("/")).await.unwrap();

    // ── assert: folder-style ─────────────────────────────────────────────────
    assert!(
        folder_listing.folders.contains(&format!("{root}/sub/")),
        "expected '{root}/sub/' in folders, got {:?}",
        folder_listing.folders
    );
    let folder_keys: Vec<&str> = folder_listing
        .objects
        .iter()
        .map(|o| o.key.as_str())
        .collect();
    assert!(
        folder_keys.contains(&format!("{root}/a.txt").as_str()),
        "expected a.txt in objects, got {folder_keys:?}"
    );
    assert!(
        folder_keys.contains(&format!("{root}/b.txt").as_str()),
        "expected b.txt in objects, got {folder_keys:?}"
    );
    assert!(
        !folder_keys.contains(&format!("{root}/sub/c.txt").as_str()),
        "sub/c.txt must not appear directly in folder listing, got {folder_keys:?}"
    );
    // sizes should be non-zero
    for obj in &folder_listing.objects {
        assert!(obj.size > 0, "expected size > 0 for {}", obj.key);
    }

    // ── act: flat listing (no delimiter) ─────────────────────────────────────
    let flat_listing = store.list(&prefix, None).await.unwrap();

    // ── assert: flat listing ─────────────────────────────────────────────────
    let flat_keys: Vec<&str> = flat_listing
        .objects
        .iter()
        .map(|o| o.key.as_str())
        .collect();
    assert!(
        flat_keys.contains(&format!("{root}/a.txt").as_str()),
        "flat: expected a.txt, got {flat_keys:?}"
    );
    assert!(
        flat_keys.contains(&format!("{root}/b.txt").as_str()),
        "flat: expected b.txt, got {flat_keys:?}"
    );
    assert!(
        flat_keys.contains(&format!("{root}/sub/c.txt").as_str()),
        "flat: expected sub/c.txt, got {flat_keys:?}"
    );
    assert!(
        flat_listing.folders.is_empty(),
        "flat listing must have no folders, got {:?}",
        flat_listing.folders
    );

    // ── cleanup ───────────────────────────────────────────────────────────────
    store.delete(&uri_a).await.unwrap();
    store.delete(&uri_b).await.unwrap();
    store.delete(&uri_c).await.unwrap();
}
