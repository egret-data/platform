//! DB-gated CatalogStore namespace tests. Self-skip without DATABASE_URL.
mod common;
use common::{seed_catalog, store};
use std::collections::HashMap;

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
