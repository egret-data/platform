//! Shared helpers for egret-catalog integration tests.
//! Each integration-test binary only uses a subset of these; suppress the
//! resulting dead_code warnings here so callers stay warning-free.
#![allow(dead_code)]

use egret_catalog_engine::CatalogStore;
use egret_metastore::Db;
use uuid::Uuid;

/// Connect to the database named by `DATABASE_URL` and return a ready
/// `CatalogStore`.  Returns `None` (self-skip) when the env var is unset.
pub async fn store() -> Option<CatalogStore> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(CatalogStore::new(db))
}

/// Seed a fresh workspace row and return its id.
pub async fn seed_workspace(s: &CatalogStore) -> Uuid {
    seed_workspace_with_slug(s).await.0
}

/// Seed a fresh workspace row and return `(id, slug)`.
pub async fn seed_workspace_with_slug(s: &CatalogStore) -> (Uuid, String) {
    let id = Uuid::new_v4();
    let slug = format!("ws_{}", id.simple());
    sqlx::query("INSERT INTO workspaces (id, slug, name, status) VALUES ($1, $2, $3, 'active')")
        .bind(id)
        .bind(&slug)
        .bind(&slug)
        .execute(s.pool())
        .await
        .expect("seed workspace");
    (id, slug)
}
