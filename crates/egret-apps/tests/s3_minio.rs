//! Task 7 (S1-7): `AppStore`'s full lifecycle against a REAL object store, not
//! `MemStore`. Every other test in this crate proves the logic is right against
//! `MemStore`; this is the one place that proves the same code also works
//! against the actual backend this plan exists to migrate to — the same
//! discipline `egret-storage/tests/s3_minio.rs` already applies to `ObjectStore`
//! itself. Skips (reporting `ok` via early return, not a real pass) unless
//! `EGRET_TEST_S3_ENDPOINT` is set, e.g. against MinIO via `just up`.

use egret_apps::store::{AppStore, PublishOutcome, SaveOutcome};
use egret_core::{UserId, WorkspaceId};
use egret_metastore::Db;
use egret_storage::{ObjectStore, S3Store, StorageConfig};
use std::sync::Arc;
use uuid::Uuid;

fn storage_cfg() -> Option<StorageConfig> {
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
async fn app_lifecycle_round_trips_through_real_s3() {
    let Some(cfg) = storage_cfg() else {
        eprintln!("EGRET_TEST_S3_ENDPOINT unset — skipping");
        return;
    };
    let url = egret_test_db::url().await.expect("test database");
    let db = Db::connect(&url, 5).await.expect("connect");
    let tag = Uuid::new_v4().simple().to_string();
    let user: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, status, password_hash) \
         VALUES ('local', $1, 'test', 'active', 'x') RETURNING id",
    )
    .bind(format!("apps_s3_{tag}@egret.local"))
    .fetch_one(&db.pool)
    .await
    .expect("insert user");
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'Apps S3 Live Test', $2, 'active') RETURNING id",
    )
    .bind(format!("apps-s3-{tag}"))
    .bind(user)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");

    let bucket = cfg.bucket.clone();
    let prefix = format!("egret-test/apps-{tag}/");
    let store: Arc<dyn ObjectStore> = Arc::new(S3Store::new(&cfg).await.expect("connect to S3"));
    let app_store = AppStore::new(db.clone(), store.clone(), bucket.clone(), prefix.clone());

    let ws_id = WorkspaceId(ws);
    let user_id = UserId(user);

    // Create -> the draft's source is readable back, from S3.
    let app = app_store
        .create(ws_id, user_id, "s3 live test", "", None, "x = 1")
        .await
        .expect("create");
    let draft = app_store.draft(ws_id, app.id).await.unwrap().unwrap();
    assert_eq!(draft.source, "x = 1");
    let draft_key = format!(
        "s3://{bucket}/{prefix}{}/{}/draft/1.py",
        ws.hyphenated(),
        app.id.hyphenated()
    );
    assert_eq!(store.get(&draft_key).await.unwrap(), b"x = 1");

    // Save with a layout -> round-trips through S3 too.
    let layout = r#"{"type":"grid","data":{}}"#;
    match app_store
        .save_draft(
            ws_id,
            app.id,
            user_id,
            draft.revision,
            "x = 2",
            Some(layout),
        )
        .await
        .unwrap()
    {
        SaveOutcome::Saved { revision } => assert_eq!(revision, draft.revision + 1),
        other => panic!("expected Saved, got {other:?}"),
    }
    let draft2 = app_store.draft(ws_id, app.id).await.unwrap().unwrap();
    assert_eq!(draft2.source, "x = 2");
    assert_eq!(draft2.layout_json.as_deref(), Some(layout));
    // Revision-keyed (Task 10): the save landed at revision 2's own keys, and
    // revision 1's object was cleaned up once the save committed.
    let rev_key = |rev: i64, file: &str| {
        format!(
            "s3://{bucket}/{prefix}{}/{}/draft/{rev}.{file}",
            ws.hyphenated(),
            app.id.hyphenated()
        )
    };
    assert_eq!(store.get(&rev_key(2, "py")).await.unwrap(), b"x = 2");
    assert_eq!(
        store.get(&rev_key(2, "layout.json")).await.unwrap(),
        layout.as_bytes()
    );
    assert!(matches!(
        store.get(&rev_key(1, "py")).await,
        Err(egret_storage::StorageError::NotFound(_))
    ));

    // Publish -> the version's own S3 key holds the draft's content.
    let v1 = match app_store
        .publish(ws_id, app.id, user_id, None)
        .await
        .unwrap()
    {
        PublishOutcome::Published(v) => v,
        other => panic!("expected Published, got {other:?}"),
    };
    let v1_key = format!(
        "s3://{bucket}/{prefix}{}/{}/versions/{}/source.py",
        ws.hyphenated(),
        app.id.hyphenated(),
        v1.version_no
    );
    assert_eq!(store.get(&v1_key).await.unwrap(), b"x = 2");

    // Save again, publish again, then restore v1 -> a server-side copy, proven
    // by the new version's key holding v1's content without this test ever
    // reading v1's bytes itself.
    app_store
        .save_draft(ws_id, app.id, user_id, draft2.revision, "x = 3", None)
        .await
        .unwrap();
    app_store
        .publish(ws_id, app.id, user_id, None)
        .await
        .unwrap();
    let v3 = app_store
        .restore_version(ws_id, app.id, v1.id, user_id)
        .await
        .unwrap()
        .expect("v1 exists");
    let v3_key = format!(
        "s3://{bucket}/{prefix}{}/{}/versions/{}/source.py",
        ws.hyphenated(),
        app.id.hyphenated(),
        v3.version_no
    );
    assert_eq!(
        store.get(&v3_key).await.unwrap(),
        b"x = 2",
        "restore must copy v1's content, not v2's or v3's draft"
    );

    // Cleanup: this test's own prefix only, never touching anything else in
    // the shared bucket.
    let _ = store
        .delete_prefix(&format!("s3://{bucket}/{prefix}"))
        .await;
}
