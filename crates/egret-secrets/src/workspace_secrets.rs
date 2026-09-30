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
    /// References to this secret: storage and database connections, registry
    /// catalogs, and engine catalog credentials.
    pub used_by: i64,
    /// k8s-style secret type (Opaque / kubernetes.io/tls / kubernetes.io/basic-auth).
    pub r#type: String,
    /// Data key names (NOT values).
    pub keys: Vec<String>,
}

/// One `workspace_secrets` row joined with its type, key names, and reference count.
type MetaRow = (
    Uuid,
    String,
    Option<String>,
    DateTime<Utc>,
    i64,
    String,
    Vec<String>,
);

impl From<MetaRow> for WorkspaceSecretMeta {
    fn from((id, name, description, created_at, used_by, r#type, keys): MetaRow) -> Self {
        WorkspaceSecretMeta {
            id,
            name,
            description,
            created_at,
            used_by,
            r#type,
            keys,
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

    /// Create a typed named secret (validates + seals the data map into a new
    /// vault row, then registers the named entry). Duplicate name → rollback.
    pub async fn create_typed(
        &self,
        workspace_id: Option<Uuid>,
        name: &str,
        description: Option<&str>,
        r#type: &str,
        data: &std::collections::BTreeMap<String, String>,
    ) -> Result<Uuid, SecretsError> {
        let vault_id = self
            .secrets
            .put_typed("workspace_secret", r#type, data)
            .await?;
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
                let _ = self.secrets.delete(vault_id).await;
                if is_unique_violation(&e) {
                    Err(SecretsError::AlreadyExists(name.to_string()))
                } else {
                    Err(e.into())
                }
            }
        }
    }

    /// Rotate a named secret's type + data map (re-validates; the named id is unchanged).
    pub async fn update_typed(
        &self,
        workspace_id: Option<Uuid>,
        id: Uuid,
        r#type: &str,
        data: &std::collections::BTreeMap<String, String>,
    ) -> Result<(), SecretsError> {
        let secret_ref = self.secret_ref(workspace_id, id).await?;
        self.secrets.update_typed(secret_ref, r#type, data).await
    }

    /// List a workspace's named secrets with reference counts, ordered by name.
    pub async fn list(
        &self,
        workspace_id: Option<Uuid>,
    ) -> Result<Vec<WorkspaceSecretMeta>, SecretsError> {
        let rows: Vec<MetaRow> = sqlx::query_as(
            "SELECT ws.id, ws.name, ws.description, ws.created_at, \
             (SELECT count(*) FROM storage.storage_connections WHERE secret_ref = ws.id) \
             + (SELECT count(*) FROM query.databases WHERE secret_id = ws.id) \
             + (SELECT count(*) FROM registry.catalog_secret_refs WHERE secret_id = ws.id) \
             + (SELECT count(*) FROM registry.engine_catalog_attachments WHERE credential_secret_id = ws.id) AS used_by, \
             s.type, \
             ARRAY(SELECT key FROM secret_data WHERE secret_id = ws.secret_ref ORDER BY key) AS keys \
             FROM workspace_secrets ws JOIN secrets s ON s.id = ws.secret_ref \
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
             + (SELECT count(*) FROM query.databases WHERE secret_id = ws.id) \
             + (SELECT count(*) FROM registry.catalog_secret_refs WHERE secret_id = ws.id) \
             + (SELECT count(*) FROM registry.engine_catalog_attachments WHERE credential_secret_id = ws.id) AS used_by, \
             s.type, \
             ARRAY(SELECT key FROM secret_data WHERE secret_id = ws.secret_ref ORDER BY key) AS keys \
             FROM workspace_secrets ws JOIN secrets s ON s.id = ws.secret_ref \
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

    /// Decrypt and return a named secret as its typed form (type + data map).
    pub async fn resolve_typed(
        &self,
        workspace_id: Option<Uuid>,
        id: Uuid,
    ) -> Result<crate::store::TypedSecret, SecretsError> {
        let secret_ref = self.secret_ref(workspace_id, id).await?;
        self.secrets.get_typed(secret_ref).await
    }

    /// Decrypt a named secret by its NAME, in `workspace_id` only.
    ///
    /// **Workspace-scoped ONLY: never the global (NULL-workspace) fallback.**
    /// The name comes from a flow author's `{{ secret('NAME') }}` (W5b), so it
    /// is user-supplied in exactly the sense `resolve_plaintext_ws_or_global`'s
    /// precondition forbids: a fallback would let any flow read a platform
    /// secret by guessing its name. Names are `UNIQUE (workspace_id, name)`
    /// (migration 0032), so this is at most one row.
    pub async fn resolve_typed_by_name(
        &self,
        workspace_id: Uuid,
        name: &str,
    ) -> Result<crate::store::TypedSecret, SecretsError> {
        let secret_ref: Option<Uuid> = sqlx::query_scalar(
            "SELECT secret_ref FROM workspace_secrets WHERE workspace_id = $1 AND name = $2",
        )
        .bind(workspace_id)
        .bind(name)
        .fetch_optional(&self.db.pool)
        .await?;
        let secret_ref = secret_ref.ok_or(SecretsError::NotFound)?;
        self.secrets.get_typed(secret_ref).await
    }

    /// Resolve a secret readable from `workspace_id`: its own workspace secret, or a
    /// GLOBAL (workspace_id NULL) secret. Used for platform-provisioned secrets (e.g.
    /// the per-engine delegator credential) that a workspace database references.
    ///
    /// SECURITY PRECONDITION: `id` must be an egret-SET `secret_id` (e.g. from a
    /// provisioned `query.databases` row materialized by egret), never a raw
    /// user-supplied secret id. The GLOBAL fallback would otherwise let a caller
    /// read any platform secret by id. User-supplied secret ids MUST instead go
    /// through the workspace-scoped `resolve_plaintext`/`get_meta` (which excludes
    /// NULL-workspace secrets). Only egret-controlled, read-only, coordinator-hosted
    /// connection rows may carry a global `secret_id`.
    pub async fn resolve_plaintext_ws_or_global(
        &self,
        workspace_id: Uuid,
        id: Uuid,
    ) -> Result<String, SecretsError> {
        let secret_ref: Option<Uuid> = sqlx::query_scalar(
            "SELECT secret_ref FROM workspace_secrets \
             WHERE id = $1 AND (workspace_id = $2 OR workspace_id IS NULL)",
        )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(&self.db.pool)
        .await?;
        let secret_ref = secret_ref.ok_or(SecretsError::NotFound)?;
        self.secrets.get(secret_ref).await
    }

    /// Read a named secret as a `(username, password)` pair. A
    /// `kubernetes.io/basic-auth` secret yields its two keys; any other (legacy
    /// Opaque) secret yields `("", <single value>)` so callers keep working before
    /// the P1b backfill converts it.
    pub async fn resolve_basic_auth(
        &self,
        workspace_id: Option<Uuid>,
        id: Uuid,
    ) -> Result<(String, String), SecretsError> {
        let typed = self.resolve_typed(workspace_id, id).await?;
        Ok(split_basic_auth(typed))
    }

    /// Like `resolve_basic_auth` but resolves a workspace-scoped OR global secret
    /// (mirrors `resolve_plaintext_ws_or_global`; same security precondition — `id`
    /// must be an egret-set `secret_id`, never user-supplied).
    pub async fn resolve_basic_auth_ws_or_global(
        &self,
        workspace_id: Uuid,
        id: Uuid,
    ) -> Result<(String, String), SecretsError> {
        let secret_ref: Option<Uuid> = sqlx::query_scalar(
            "SELECT secret_ref FROM workspace_secrets \
             WHERE id = $1 AND (workspace_id = $2 OR workspace_id IS NULL)",
        )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(&self.db.pool)
        .await?;
        let secret_ref = secret_ref.ok_or(SecretsError::NotFound)?;
        Ok(split_basic_auth(self.secrets.get_typed(secret_ref).await?))
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

/// Split a resolved typed secret into a `(username, password)` pair. A
/// `kubernetes.io/basic-auth` secret maps its two keys; any other (legacy Opaque)
/// secret returns its single `value` under the password slot with an empty username.
fn split_basic_auth(typed: crate::store::TypedSecret) -> (String, String) {
    if typed.r#type == crate::secret_type::BASIC_AUTH {
        (
            typed.data.get("username").cloned().unwrap_or_default(),
            typed.data.get("password").cloned().unwrap_or_default(),
        )
    } else {
        (
            String::new(),
            typed.data.get("value").cloned().unwrap_or_default(),
        )
    }
}

/// Whether a sqlx error is a Postgres unique-constraint violation (SQLSTATE 23505).
fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if db.code().as_deref() == Some("23505"))
}
