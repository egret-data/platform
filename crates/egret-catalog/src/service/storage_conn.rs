//! Storage connection registry: per-workspace N-entry storage registry.
//!
//! Provides CRUD on `CatalogStore` targeting the `workspace_storages` table
//! created in migration 0030. All mutations are workspace-scoped to prevent
//! cross-tenant IDOR: every query includes both `id = $` and `workspace_id = $`.

use crate::CatalogError;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::store::CatalogStore;

/// A registered storage connection entry in `workspace_storages`.
/// No secret plaintext value is stored; `secret_ref` is a foreign key into
/// whatever vault/secret table the caller manages.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkspaceStorageConn {
    pub id: Uuid,
    pub workspace_id: Uuid,
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
    workspace_id: Uuid,
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

impl From<StorageConnRecord> for WorkspaceStorageConn {
    fn from(r: StorageConnRecord) -> Self {
        WorkspaceStorageConn {
            id: r.id,
            workspace_id: r.workspace_id,
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

impl CatalogStore {
    /// Insert a new storage connection for the workspace. Returns the created row.
    /// Errors with `CatalogError::Db` on unique-name violation or other DB errors.
    pub async fn create_storage_conn(
        &self,
        workspace_id: Uuid,
        input: &StorageConnInput,
    ) -> Result<WorkspaceStorageConn, CatalogError> {
        let rec = sqlx::query_as::<_, StorageConnRecord>(
            "INSERT INTO workspace_storages \
               (workspace_id, name, purpose, provider, endpoint, region, bucket, \
                key_prefix, access_key, secret_ref, path_style) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) \
             RETURNING id, workspace_id, name, purpose, provider, endpoint, region, \
                       bucket, key_prefix, access_key, secret_ref, path_style, created_at",
        )
        .bind(workspace_id)
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
        .map_err(CatalogError::Db)?;

        Ok(rec.into())
    }

    /// List all storage connections for a workspace, ordered by creation time.
    pub async fn list_storage_conns(
        &self,
        workspace_id: Uuid,
    ) -> Result<Vec<WorkspaceStorageConn>, CatalogError> {
        let recs = sqlx::query_as::<_, StorageConnRecord>(
            "SELECT id, workspace_id, name, purpose, provider, endpoint, region, \
                    bucket, key_prefix, access_key, secret_ref, path_style, created_at \
             FROM workspace_storages \
             WHERE workspace_id = $1 \
             ORDER BY created_at",
        )
        .bind(workspace_id)
        .fetch_all(self.pool())
        .await
        .map_err(CatalogError::Db)?;

        Ok(recs.into_iter().map(Into::into).collect())
    }

    /// Get a single storage connection by id, scoped to the workspace.
    /// Returns `None` if the id does not exist or belongs to a different workspace.
    pub async fn get_storage_conn(
        &self,
        workspace_id: Uuid,
        id: Uuid,
    ) -> Result<Option<WorkspaceStorageConn>, CatalogError> {
        let rec = sqlx::query_as::<_, StorageConnRecord>(
            "SELECT id, workspace_id, name, purpose, provider, endpoint, region, \
                    bucket, key_prefix, access_key, secret_ref, path_style, created_at \
             FROM workspace_storages \
             WHERE id = $1 AND workspace_id = $2",
        )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(self.pool())
        .await
        .map_err(CatalogError::Db)?;

        Ok(rec.map(Into::into))
    }

    /// Update a storage connection. Returns `true` if a row was updated,
    /// `false` if no row matched (wrong workspace or non-existent id — no-op).
    pub async fn update_storage_conn(
        &self,
        workspace_id: Uuid,
        id: Uuid,
        input: &StorageConnInput,
    ) -> Result<bool, CatalogError> {
        let result = sqlx::query(
            "UPDATE workspace_storages SET \
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
             WHERE id = $11 AND workspace_id = $12",
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
        .bind(workspace_id)
        .execute(self.pool())
        .await
        .map_err(CatalogError::Db)?;

        Ok(result.rows_affected() == 1)
    }

    /// Delete a storage connection. Returns `true` if a row was deleted,
    /// `false` if no row matched (wrong workspace or non-existent id — no-op).
    pub async fn delete_storage_conn(
        &self,
        workspace_id: Uuid,
        id: Uuid,
    ) -> Result<bool, CatalogError> {
        let result =
            sqlx::query("DELETE FROM workspace_storages WHERE id = $1 AND workspace_id = $2")
                .bind(id)
                .bind(workspace_id)
                .execute(self.pool())
                .await
                .map_err(CatalogError::Db)?;

        Ok(result.rows_affected() == 1)
    }
}
