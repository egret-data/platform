//! Postgres-backed `CatalogStore`: the shared handle the storage-connection
//! registry ([`storage_conn`](super::storage_conn)) hangs its queries off.
//! Holds the egret-side sqlx pool via [`egret_metastore::Db`].

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
