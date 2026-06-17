//! DB-gated CatalogStore namespace tests. Self-skip without DATABASE_URL.
use egret_catalog::service::CatalogStore;
use egret_metastore::Db;
use std::collections::HashMap;

async fn store() -> Option<CatalogStore> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(CatalogStore::new(db))
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
    let levels = ns();
    s.create_namespace(&levels, &HashMap::from([("k".into(), "v".into())]))
        .await
        .unwrap();
    let got = s.get_namespace(&levels).await.unwrap();
    assert_eq!(got.levels, levels);
    s.drop_namespace(&levels).await.unwrap();
    assert!(s.get_namespace(&levels).await.is_err());
}

#[tokio::test]
async fn duplicate_namespace_is_already_exists() {
    let Some(s) = store().await else {
        eprintln!("skip");
        return;
    };
    let levels = ns();
    s.create_namespace(&levels, &HashMap::new()).await.unwrap();
    let err = s
        .create_namespace(&levels, &HashMap::new())
        .await
        .unwrap_err();
    assert!(matches!(err, egret_catalog::CatalogError::AlreadyExists(_)));
    s.drop_namespace(&levels).await.unwrap();
}
