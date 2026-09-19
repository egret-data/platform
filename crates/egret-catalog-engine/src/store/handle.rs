//! Postgres-backed `CatalogStore`: the shared handle the embedded-catalog
//! stores ([`embedded`](super::embedded): warehouses, roles/grants, clients)
//! hang their queries off. Holds the egret-side sqlx pool via
//! [`egret_metastore::Db`].

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

    /// The catalog registry on the same database (SP3).
    pub fn registry(&self) -> egret_catalog_registry::RegistryStore {
        egret_catalog_registry::RegistryStore::new(self.db.clone())
    }
}
