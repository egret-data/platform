//! DB-gated catalogs-store tests. Self-skip without DATABASE_URL.
mod common;
use common::{seed_workspace, seed_workspace_with_slug, store};

#[tokio::test]
async fn create_and_get_catalog() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    let c = s
        .create_catalog(ws, "main", "s3://wh/workspaces/ws/warehouse")
        .await
        .unwrap();
    assert_eq!(c.workspace_id, ws);
    assert_eq!(c.warehouse_location, "s3://wh/workspaces/ws/warehouse");
    let got = s.get_catalog_by_workspace(ws).await.unwrap();
    assert_eq!(got.id, c.id);
    let by_id = s.get_catalog_by_id(c.id).await.unwrap();
    assert_eq!(by_id.workspace_id, ws);
}

#[tokio::test]
async fn duplicate_catalog_per_workspace_is_already_exists() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    s.create_catalog(ws, "main", "s3://wh/a").await.unwrap();
    let err = s
        .create_catalog(ws, "second", "s3://wh/b")
        .await
        .unwrap_err();
    assert!(
        matches!(err, egret_catalog::CatalogError::AlreadyExists(_)),
        "got: {err:?}"
    );
}

#[tokio::test]
async fn get_catalog_by_slug_resolves() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (ws, slug) = seed_workspace_with_slug(&s).await;
    let c = s
        .create_catalog(ws, "main", "s3://wh/slug-test")
        .await
        .unwrap();

    // Happy path: slug resolves to the correct catalog.
    let got = s.get_catalog_by_slug(&slug).await.unwrap();
    assert_eq!(got.id, c.id, "resolved catalog id must match");
    assert_eq!(got.workspace_id, ws);

    // Unknown slug returns NotFound.
    let err = s
        .get_catalog_by_slug("nonexistent-slug-xyz")
        .await
        .unwrap_err();
    assert!(
        matches!(err, egret_catalog::CatalogError::NotFound(_)),
        "expected NotFound, got: {err:?}"
    );
}
