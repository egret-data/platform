//! A failing ledger write must never change or fail a download. This file
//! holds ONE test because it breaks the ledger table (renames it), and every
//! test in a binary shares one database.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_metastore::Db;
use egret_policy::{ranger_router, seed_trino, PolicyStore, RangerState};
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
async fn a_ledger_write_error_does_not_change_the_download() {
    let Some(url) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let store = PolicyStore::new(db);
    seed_trino(&store).await.expect("seed");
    let svc = format!("trino_{}", Uuid::new_v4().simple());
    store
        .create_service(&svc, "trino", &json!({}))
        .await
        .unwrap();

    let req = |uri: String| {
        Request::builder()
            .uri(uri)
            .header("authorization", "Bearer devtoken")
            .body(Body::empty())
            .unwrap()
    };
    let app = || {
        ranger_router(RangerState {
            store: store.clone(),
            auth_token: Arc::from("devtoken"),
        })
    };
    let before = |uri: String| async { app().oneshot(req(uri)).await.unwrap().status() };
    let tags = format!("/tags/download/{svc}?pluginId=trino@h-{svc}&lastKnownVersion=-1");
    let tags_304 = format!("/tags/download/{svc}?pluginId=trino@h-{svc}&lastKnownVersion=1");
    let pols =
        format!("/plugins/policies/download/{svc}?pluginId=trino@h-{svc}&lastKnownVersion=-1");
    assert_eq!(before(tags.clone()).await, StatusCode::OK);

    sqlx::query(
        "ALTER TABLE policy.ranger_plugin_downloads RENAME TO ranger_plugin_downloads_broken",
    )
    .execute(&pool)
    .await
    .unwrap();

    assert_eq!(before(tags).await, StatusCode::OK);
    assert_eq!(before(tags_304).await, StatusCode::NOT_MODIFIED);
    assert_eq!(before(pols).await, StatusCode::OK);
}
