//! Storage-connection registry: the global (control-plane) N-entry registry.
//!
//! Provides CRUD on the `storage.storage_connections` table via [`StorageRegistry`].
//! Storage connections are global (platform-admin only); they are not scoped to a
//! workspace. Entries are keyed by `id` (or by unique `name`).
//!
//! Relocated from `egret-catalog` (E3a): the Catalog seam no longer owns storage.

use chrono::{DateTime, Utc};
use egret_metastore::Db;
use uuid::Uuid;

/// Errors produced by the storage-connection registry.
#[derive(Debug, thiserror::Error)]
pub enum StorageRegistryError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// A registered storage connection entry in `storage.storage_connections`.
/// No secret plaintext value is stored; `secret_ref` is a foreign key into
/// whatever vault/secret table the caller manages.
#[derive(Clone, Debug, PartialEq)]
pub struct StorageConn {
    pub id: Uuid,
    pub name: String,
    pub purpose: Option<String>,
    pub provider: String,
    pub endpoint: Option<String>,
    pub region: String,
    pub bucket: String,
    pub key_prefix: Option<String>,
    pub access_key: String,
    pub secret_ref: Option<Uuid>,
    pub path_style: bool,
    pub created_at: DateTime<Utc>,
}

/// Input for creating or updating a storage connection entry.
#[derive(Clone, Debug)]
pub struct StorageConnInput {
    pub name: String,
    pub purpose: Option<String>,
    pub provider: String,
    pub endpoint: Option<String>,
    pub region: String,
    pub bucket: String,
    pub key_prefix: Option<String>,
    pub access_key: String,
    pub secret_ref: Option<Uuid>,
    pub path_style: bool,
}

/// Internal sqlx row type for `query_as` deserialization.
#[derive(sqlx::FromRow)]
struct StorageConnRecord {
    id: Uuid,
    name: String,
    purpose: Option<String>,
    provider: String,
    endpoint: Option<String>,
    region: String,
    bucket: String,
    key_prefix: Option<String>,
    access_key: String,
    secret_ref: Option<Uuid>,
    path_style: bool,
    created_at: DateTime<Utc>,
}

impl From<StorageConnRecord> for StorageConn {
    fn from(r: StorageConnRecord) -> Self {
        StorageConn {
            id: r.id,
            name: r.name,
            purpose: r.purpose,
            provider: r.provider,
            endpoint: r.endpoint,
            region: r.region,
            bucket: r.bucket,
            key_prefix: r.key_prefix,
            access_key: r.access_key,
            secret_ref: r.secret_ref,
            path_style: r.path_style,
            created_at: r.created_at,
        }
    }
}

/// Postgres-backed registry over `storage.storage_connections`.
#[derive(Clone)]
pub struct StorageRegistry {
    db: Db,
}

impl StorageRegistry {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    fn pool(&self) -> &sqlx::PgPool {
        &self.db.pool
    }

    /// Insert a new (global) storage connection. Returns the created row.
    /// A duplicate-name unique violation maps to `StorageRegistryError::AlreadyExists`.
    pub async fn create_storage_conn(
        &self,
        input: &StorageConnInput,
    ) -> Result<StorageConn, StorageRegistryError> {
        let rec = sqlx::query_as::<_, StorageConnRecord>(
            "INSERT INTO storage.storage_connections \
               (name, purpose, provider, endpoint, region, bucket, \
                key_prefix, access_key, secret_ref, path_style) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
             RETURNING id, name, purpose, provider, endpoint, region, \
                       bucket, key_prefix, access_key, secret_ref, path_style, created_at",
        )
        .bind(&input.name)
        .bind(input.purpose.as_deref())
        .bind(&input.provider)
        .bind(input.endpoint.as_deref())
        .bind(&input.region)
        .bind(&input.bucket)
        .bind(input.key_prefix.as_deref())
        .bind(&input.access_key)
        .bind(input.secret_ref)
        .bind(input.path_style)
        .fetch_one(self.pool())
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.constraint() == Some("storage_connections_name_key") => {
                StorageRegistryError::AlreadyExists(input.name.clone())
            }
            _ => StorageRegistryError::Db(e),
        })?;

        Ok(rec.into())
    }

    /// List all storage connections, ordered by name.
    pub async fn list_storage_conns(&self) -> Result<Vec<StorageConn>, StorageRegistryError> {
        let recs = sqlx::query_as::<_, StorageConnRecord>(
            "SELECT id, name, purpose, provider, endpoint, region, \
                    bucket, key_prefix, access_key, secret_ref, path_style, created_at \
             FROM storage.storage_connections \
             ORDER BY name",
        )
        .fetch_all(self.pool())
        .await
        .map_err(StorageRegistryError::Db)?;

        Ok(recs.into_iter().map(Into::into).collect())
    }

    /// Get a single storage connection by id. Returns `None` if it does not exist.
    pub async fn get_storage_conn(
        &self,
        id: Uuid,
    ) -> Result<Option<StorageConn>, StorageRegistryError> {
        let rec = sqlx::query_as::<_, StorageConnRecord>(
            "SELECT id, name, purpose, provider, endpoint, region, \
                    bucket, key_prefix, access_key, secret_ref, path_style, created_at \
             FROM storage.storage_connections \
             WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await
        .map_err(StorageRegistryError::Db)?;

        Ok(rec.map(Into::into))
    }

    /// Update a storage connection. Returns `true` if a row was updated,
    /// `false` if no row matched the given id (no-op).
    pub async fn update_storage_conn(
        &self,
        id: Uuid,
        input: &StorageConnInput,
    ) -> Result<bool, StorageRegistryError> {
        let result = sqlx::query(
            "UPDATE storage.storage_connections SET \
               name       = $1, \
               purpose    = $2, \
               provider   = $3, \
               endpoint   = $4, \
               region     = $5, \
               bucket     = $6, \
               key_prefix = $7, \
               access_key = $8, \
               secret_ref = $9, \
               path_style = $10 \
             WHERE id = $11",
        )
        .bind(&input.name)
        .bind(input.purpose.as_deref())
        .bind(&input.provider)
        .bind(input.endpoint.as_deref())
        .bind(&input.region)
        .bind(&input.bucket)
        .bind(input.key_prefix.as_deref())
        .bind(&input.access_key)
        .bind(input.secret_ref)
        .bind(input.path_style)
        .bind(id)
        .execute(self.pool())
        .await
        .map_err(StorageRegistryError::Db)?;

        Ok(result.rows_affected() == 1)
    }

    /// Delete a storage connection. Returns `true` if a row was deleted,
    /// `false` if no row matched the given id (no-op).
    pub async fn delete_storage_conn(&self, id: Uuid) -> Result<bool, StorageRegistryError> {
        let result = sqlx::query("DELETE FROM storage.storage_connections WHERE id = $1")
            .bind(id)
            .execute(self.pool())
            .await
            .map_err(StorageRegistryError::Db)?;

        Ok(result.rows_affected() == 1)
    }
}
