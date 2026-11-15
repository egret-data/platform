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

/// Seed a service account (a `provider='service'` user plus its
/// `service_accounts` row) in `workspace_id` and return its user id.
pub async fn seed_service_account(s: &CatalogStore, workspace_id: Uuid) -> Uuid {
    let suffix = Uuid::new_v4().simple().to_string();
    let sa: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name) VALUES ('service', $1, $2) RETURNING id",
    )
    .bind(format!("sa-{suffix}@service.invalid"))
    .bind(format!("sa-{suffix}"))
    .fetch_one(s.pool())
    .await
    .expect("seed service user");
    sqlx::query(
        "INSERT INTO service_accounts (user_id, scope, workspace_id, name) \
         VALUES ($1, 'workspace', $2, $3)",
    )
    .bind(sa)
    .bind(workspace_id)
    .bind(format!("sa-{suffix}"))
    .execute(s.pool())
    .await
    .expect("seed service account");
    sa
}
