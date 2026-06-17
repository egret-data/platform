//! DB-gated view-store tests. Self-skip without DATABASE_URL.
use egret_catalog::service::CatalogStore;
use egret_metastore::Db;
use std::collections::HashMap;

async fn store() -> Option<CatalogStore> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(CatalogStore::new(db))
}

#[tokio::test]
async fn insert_load_drop_view() {
    let Some(s) = store().await else {
        eprintln!("DATABASE_URL not set — skipping store_views test");
        return;
    };
    let levels = vec![format!("t_{}", uuid::Uuid::new_v4().simple())];
    let ns = s
        .create_namespace(&levels, &HashMap::new())
        .await
        .unwrap();
    let md = serde_json::json!({ "view-uuid": uuid::Uuid::new_v4().to_string() });
    s.insert_view(
        ns.id,
        "v",
        &md,
        "s3://wh/ns/v/metadata/0.json",
        "s3://wh/ns/v",
    )
    .await
    .unwrap();
    assert_eq!(s.load_view(ns.id, "v").await.unwrap().name, "v");
    assert_eq!(
        s.list_views(ns.id).await.unwrap(),
        vec!["v".to_string()]
    );
    s.drop_view(ns.id, "v").await.unwrap();
    assert!(s.load_view(ns.id, "v").await.is_err());
    s.drop_namespace(&levels).await.unwrap();
}
