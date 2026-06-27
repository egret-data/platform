//! Per-workspace S3 storage config store.
//!
//! Provides `upsert_workspace_storage` and `workspace_storage_for` on
//! `CatalogStore`. Targets the `workspace_storage` table created in
//! migration 0029.

use crate::CatalogError;
use uuid::Uuid;

use super::store::CatalogStore;

/// Per-workspace S3 storage configuration persisted in `workspace_storage`.
/// No secret plaintext value is stored here; `secret_ref` is a foreign key
/// into whatever vault/secret table the caller manages.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkspaceStorageRow {
    pub workspace_id: Uuid,
    pub provider: String,
    pub endpoint: Option<String>,
    pub region: String,
    pub bucket: String,
    pub key_prefix: Option<String>,
    pub access_key: String,
    pub secret_ref: Option<Uuid>,
    pub path_style: bool,
}

impl CatalogStore {
    /// Persist (or update) S3 storage config for a workspace. Idempotent:
    /// if a row already exists for `workspace_id` all mutable columns are
    /// overwritten with the new values.
    pub async fn upsert_workspace_storage(
        &self,
        row: &WorkspaceStorageRow,
    ) -> Result<(), CatalogError> {
        sqlx::query(
            "INSERT INTO workspace_storage \
               (workspace_id, provider, endpoint, region, bucket, key_prefix, \
                access_key, secret_ref, path_style) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
             ON CONFLICT (workspace_id) DO UPDATE SET \
               provider   = EXCLUDED.provider, \
               endpoint   = EXCLUDED.endpoint, \
               region     = EXCLUDED.region, \
               bucket     = EXCLUDED.bucket, \
               key_prefix = EXCLUDED.key_prefix, \
               access_key = EXCLUDED.access_key, \
               secret_ref = EXCLUDED.secret_ref, \
               path_style = EXCLUDED.path_style",
        )
        .bind(row.workspace_id)
        .bind(&row.provider)
        .bind(row.endpoint.as_deref())
        .bind(&row.region)
        .bind(&row.bucket)
        .bind(row.key_prefix.as_deref())
        .bind(&row.access_key)
        .bind(row.secret_ref)
        .bind(row.path_style)
        .execute(self.pool())
        .await
        .map_err(CatalogError::Db)?;
        Ok(())
    }

    /// Return the S3 storage config for `workspace_id`, or `None` if not yet
    /// configured.
    pub async fn workspace_storage_for(
        &self,
        workspace_id: Uuid,
    ) -> Result<Option<WorkspaceStorageRow>, CatalogError> {
        let row = sqlx::query_as::<_, WorkspaceStorageRowRecord>(
            "SELECT workspace_id, provider, endpoint, region, bucket, key_prefix, \
                    access_key, secret_ref, path_style \
             FROM workspace_storage \
             WHERE workspace_id = $1",
        )
        .bind(workspace_id)
        .fetch_optional(self.pool())
        .await
        .map_err(CatalogError::Db)?;

        Ok(row.map(|r| WorkspaceStorageRow {
            workspace_id: r.workspace_id,
            provider: r.provider,
            endpoint: r.endpoint,
            region: r.region,
            bucket: r.bucket,
            key_prefix: r.key_prefix,
            access_key: r.access_key,
            secret_ref: r.secret_ref,
            path_style: r.path_style,
        }))
    }
}

/// Internal sqlx row type for `query_as` deserialization.
#[derive(sqlx::FromRow)]
struct WorkspaceStorageRowRecord {
    workspace_id: Uuid,
    provider: String,
    endpoint: Option<String>,
    region: String,
    bucket: String,
    key_prefix: Option<String>,
    access_key: String,
    secret_ref: Option<Uuid>,
    path_style: bool,
}
