//! DB-gated catalogs-store tests. Self-skip without DATABASE_URL.
use egret_catalog::service::CatalogStore;
use egret_metastore::Db;

async fn store() -> Option<CatalogStore> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(CatalogStore::new(db))
}

/// Insert a workspace row directly (FK target) and return its id.
async fn seed_workspace(s: &CatalogStore) -> uuid::Uuid {
    let id = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO workspaces (id, slug, name, status) VALUES ($1,$2,$3,'active')")
        .bind(id)
        .bind(format!("ws_{}", id.simple()))
        .bind("Test WS")
        .execute(s.pool())
        .await
        .unwrap();
    id
}

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
