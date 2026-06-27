//! Postgres-backed `CatalogStore`: the shared handle the identity-layer modules
//! ([`tokens`](super::tokens), [`warehouse`](super::warehouse)) hang their queries
//! off. Holds the egret-side sqlx pool via [`egret_metastore::Db`].

use egret_metastore::Db;

#[derive(Clone)]
pub struct CatalogStore {
    db: Db,
}

impl CatalogStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub fn pool(&self) -> &sqlx::PgPool {
        &self.db.pool
    }
}
