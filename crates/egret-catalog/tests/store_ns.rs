//! DB-gated CatalogStore namespace tests. Self-skip without DATABASE_URL.
use egret_catalog::service::CatalogStore;
use egret_metastore::Db;
use std::collections::HashMap;
use uuid::Uuid;

async fn store() -> Option<CatalogStore> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(CatalogStore::new(db))
}

/// Seed a fresh workspace and return its id.
async fn seed_workspace(s: &CatalogStore) -> Uuid {
    let id = Uuid::new_v4();
    let slug = format!("ws_{}", id.simple());
    sqlx::query("INSERT INTO workspaces (id, slug, name, status) VALUES ($1, $2, $3, 'active')")
        .bind(id)
        .bind(&slug)
        .bind(&slug)
        .execute(s.pool())
        .await
        .expect("seed workspace");
    id
}

/// Seed a workspace + its default catalog and return the catalog id.
async fn seed_catalog(s: &CatalogStore) -> Uuid {
    let ws = seed_workspace(s).await;
    s.create_catalog(ws, "default", "s3://wh")
        .await
        .expect("seed catalog")
        .id
}

fn ns() -> Vec<String> {
    vec![format!("t_{}", uuid::Uuid::new_v4().simple())]
}

#[tokio::test]
async fn create_get_drop_namespace() {
    let Some(s) = store().await else {
        eprintln!("skip");
        return;
    };
    let cat = seed_catalog(&s).await;
    let levels = ns();
    s.create_namespace(cat, &levels, &HashMap::from([("k".into(), "v".into())]))
        .await
        .unwrap();
    let got = s.get_namespace(cat, &levels).await.unwrap();
    assert_eq!(got.levels, levels);
    s.drop_namespace(cat, &levels).await.unwrap();
    assert!(s.get_namespace(cat, &levels).await.is_err());
}

#[tokio::test]
async fn duplicate_namespace_is_already_exists() {
    let Some(s) = store().await else {
        eprintln!("skip");
        return;
    };
    let cat = seed_catalog(&s).await;
    let levels = ns();
    s.create_namespace(cat, &levels, &HashMap::new())
        .await
        .unwrap();
    let err = s
        .create_namespace(cat, &levels, &HashMap::new())
        .await
        .unwrap_err();
    assert!(matches!(err, egret_catalog::CatalogError::AlreadyExists(_)));
    s.drop_namespace(cat, &levels).await.unwrap();
}
