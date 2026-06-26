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

/// Seed a workspace + its default catalog and return the catalog id.
async fn seed_catalog(s: &CatalogStore) -> uuid::Uuid {
    let ws = uuid::Uuid::new_v4();
    let slug = format!("ws_{}", ws.simple());
    sqlx::query("INSERT INTO workspaces (id, slug, name, status) VALUES ($1, $2, $3, 'active')")
        .bind(ws)
        .bind(&slug)
        .bind(&slug)
        .execute(s.pool())
        .await
        .expect("seed workspace");
    s.create_catalog(ws, "default", "s3://wh")
        .await
        .expect("seed catalog")
        .id
}

#[tokio::test]
async fn insert_load_drop_view() {
    let Some(s) = store().await else {
        eprintln!("DATABASE_URL not set — skipping store_views test");
        return;
    };
    let cat = seed_catalog(&s).await;
    let levels = vec![format!("t_{}", uuid::Uuid::new_v4().simple())];
    let ns = s
        .create_namespace(cat, &levels, &HashMap::new())
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
    assert_eq!(s.list_views(ns.id).await.unwrap(), vec!["v".to_string()]);
    s.drop_view(ns.id, "v").await.unwrap();
    assert!(s.load_view(ns.id, "v").await.is_err());
    s.drop_namespace(cat, &levels).await.unwrap();
}
