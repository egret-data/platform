//! Named, reusable per-workspace secrets — the layer above the AES-GCM vault.
//!
//! A `workspace_secrets` row gives a vault secret a human name and tracks how many
//! connections reference it. Storage and database connections point at the named
//! row (`secret_ref` / `secret_id`); rotation re-seals the underlying vault row so
//! the referencing connections immediately read the new value.

use chrono::{DateTime, Utc};
use egret_metastore::Db;
use serde::Deserialize;
use uuid::Uuid;

use crate::error::SecretsError;
use crate::store::SecretStore;

/// Request shape for creating a brand-new named secret inline while registering a
/// connection (storage or database): `{ "name": ..., "value": ... }`. The value
/// is plaintext, accepted only on write and never returned. Shared across the
/// HTTP layers so each connection type can mint a named secret in one request.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct NewSecret {
    pub name: String,
    /// Plaintext secret value; accepted only on write, never returned.
    pub value: String,
}

/// Metadata for a named workspace secret. Never exposes the plaintext value.
#[derive(Debug, Clone)]
pub struct WorkspaceSecretMeta {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    /// Number of connections (storages + databases) referencing this secret.
    pub used_by: i64,
}

/// One `workspace_secrets` row joined with its reference count, as selected from SQL.
type MetaRow = (Uuid, String, Option<String>, DateTime<Utc>, i64);

impl From<MetaRow> for WorkspaceSecretMeta {
    fn from((id, name, description, created_at, used_by): MetaRow) -> Self {
        WorkspaceSecretMeta {
            id,
            name,
            description,
            created_at,
            used_by,
        }
    }
}

/// Scoped store for named secrets. Every method is scoped by an optional
/// `workspace_id`: `Some(ws)` targets that workspace's secrets, while `None`
/// targets global / control-plane secrets (rows with a NULL `workspace_id`).
/// The scope filter is NULL-safe (`IS NOT DISTINCT FROM`), so an id belonging
/// to another scope resolves to `NotFound`.
#[derive(Clone)]
pub struct WorkspaceSecretStore {
    db: Db,
    secrets: SecretStore,
}

impl WorkspaceSecretStore {
    pub fn new(db: Db, secrets: SecretStore) -> Self {
        Self { db, secrets }
    }

    /// Seal `plaintext` into a new vault row and register a named entry for it.
    /// On a duplicate name the vault row is rolled back and `AlreadyExists` returned.
    pub async fn create(
        &self,
        workspace_id: Option<Uuid>,
        name: &str,
        description: Option<&str>,
        plaintext: &str,
    ) -> Result<Uuid, SecretsError> {
        let vault_id = self.secrets.put("workspace_secret", plaintext).await?;
        let res: Result<Uuid, sqlx::Error> = sqlx::query_scalar(
            "INSERT INTO workspace_secrets (workspace_id, name, description, secret_ref) \
             VALUES ($1, $2, $3, $4) RETURNING id",
        )
        .bind(workspace_id)
        .bind(name)
        .bind(description)
        .bind(vault_id)
        .fetch_one(&self.db.pool)
        .await;
        match res {
            Ok(id) => Ok(id),
            Err(e) => {
                // Roll back the vault row so we don't leak orphaned ciphertext.
                let _ = self.secrets.delete(vault_id).await;
                if is_unique_violation(&e) {
                    Err(SecretsError::AlreadyExists(name.to_string()))
                } else {
                    Err(e.into())
                }
            }
        }
    }

    /// List a workspace's named secrets with reference counts, ordered by name.
    pub async fn list(
        &self,
        workspace_id: Option<Uuid>,
    ) -> Result<Vec<WorkspaceSecretMeta>, SecretsError> {
        let rows: Vec<MetaRow> = sqlx::query_as(
            "SELECT ws.id, ws.name, ws.description, ws.created_at, \
             (SELECT count(*) FROM storage.storage_connections WHERE secret_ref = ws.id) \
             + (SELECT count(*) FROM query.databases WHERE secret_id = ws.id) AS used_by \
             FROM workspace_secrets ws \
             WHERE ws.workspace_id IS NOT DISTINCT FROM $1 \
             ORDER BY ws.name",
        )
        .bind(workspace_id)
        .fetch_all(&self.db.pool)
        .await?;
        Ok(rows.into_iter().map(WorkspaceSecretMeta::from).collect())
    }

    /// Fetch metadata for a single named secret in the workspace (404 if absent).
    pub async fn get_meta(
        &self,
        workspace_id: Option<Uuid>,
        id: Uuid,
    ) -> Result<WorkspaceSecretMeta, SecretsError> {
        let row: Option<MetaRow> = sqlx::query_as(
            "SELECT ws.id, ws.name, ws.description, ws.created_at, \
             (SELECT count(*) FROM storage.storage_connections WHERE secret_ref = ws.id) \
             + (SELECT count(*) FROM query.databases WHERE secret_id = ws.id) AS used_by \
             FROM workspace_secrets ws \
             WHERE ws.id = $1 AND ws.workspace_id IS NOT DISTINCT FROM $2",
        )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(&self.db.pool)
        .await?;
        Ok(row.ok_or(SecretsError::NotFound)?.into())
    }

    /// Rotate the underlying value (re-seal the vault row); the named id is unchanged.
    pub async fn update_value(
        &self,
        workspace_id: Option<Uuid>,
        id: Uuid,
        plaintext: &str,
    ) -> Result<(), SecretsError> {
        let secret_ref = self.secret_ref(workspace_id, id).await?;
        self.secrets.update(secret_ref, plaintext).await
    }

    /// Rename / re-describe a named secret (does not touch the value).
    pub async fn rename(
        &self,
        workspace_id: Option<Uuid>,
        id: Uuid,
        name: &str,
        description: Option<&str>,
    ) -> Result<(), SecretsError> {
        let res = sqlx::query(
            "UPDATE workspace_secrets SET name = $3, description = $4 \
             WHERE id = $1 AND workspace_id IS NOT DISTINCT FROM $2",
        )
        .bind(id)
        .bind(workspace_id)
        .bind(name)
        .bind(description)
        .execute(&self.db.pool)
        .await;
        match res {
            Ok(r) if r.rows_affected() == 0 => Err(SecretsError::NotFound),
            Ok(_) => Ok(()),
            Err(e) if is_unique_violation(&e) => Err(SecretsError::AlreadyExists(name.to_string())),
            Err(e) => Err(e.into()),
        }
    }

    /// Delete a named secret and its vault row. Blocked with `InUse` if any
    /// connection still references it.
    pub async fn delete(&self, workspace_id: Option<Uuid>, id: Uuid) -> Result<(), SecretsError> {
        let meta = self.get_meta(workspace_id, id).await?;
        if meta.used_by > 0 {
            return Err(SecretsError::InUse(meta.used_by));
        }
        let secret_ref = self.secret_ref(workspace_id, id).await?;
        sqlx::query(
            "DELETE FROM workspace_secrets WHERE id = $1 AND workspace_id IS NOT DISTINCT FROM $2",
        )
        .bind(id)
        .bind(workspace_id)
        .execute(&self.db.pool)
        .await?;
        self.secrets.delete(secret_ref).await
    }

    /// Decrypt and return the plaintext for a named secret (workspace-scoped).
    pub async fn resolve_plaintext(
        &self,
        workspace_id: Option<Uuid>,
        id: Uuid,
    ) -> Result<String, SecretsError> {
        let secret_ref = self.secret_ref(workspace_id, id).await?;
        self.secrets.get(secret_ref).await
    }

    /// Resolve the vault `secret_ref` for a workspace-scoped named secret (404 if absent).
    async fn secret_ref(&self, workspace_id: Option<Uuid>, id: Uuid) -> Result<Uuid, SecretsError> {
        let row: Option<Uuid> = sqlx::query_scalar(
            "SELECT secret_ref FROM workspace_secrets \
             WHERE id = $1 AND workspace_id IS NOT DISTINCT FROM $2",
        )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(&self.db.pool)
        .await?;
        row.ok_or(SecretsError::NotFound)
    }
}

/// Whether a sqlx error is a Postgres unique-constraint violation (SQLSTATE 23505).
fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if db.code().as_deref() == Some("23505"))
}
