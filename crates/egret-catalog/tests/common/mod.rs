//! Shared helpers for egret-catalog integration tests.
//! Each integration-test binary only uses a subset of these; suppress the
//! resulting dead_code warnings here so callers stay warning-free.
#![allow(dead_code)]

use egret_catalog::service::CatalogStore;
use egret_metastore::Db;
use uuid::Uuid;

/// Connect to the database named by `DATABASE_URL` and return a ready
/// `CatalogStore`.  Returns `None` (self-skip) when the env var is unset.
pub async fn store() -> Option<CatalogStore> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(CatalogStore::new(db))
}

/// Seed a fresh workspace row and return its id.
pub async fn seed_workspace(s: &CatalogStore) -> Uuid {
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
pub async fn seed_catalog(s: &CatalogStore) -> Uuid {
    let ws = seed_workspace(s).await;
    s.create_catalog(ws, "default", "s3://wh")
        .await
        .expect("seed catalog")
        .id
}
