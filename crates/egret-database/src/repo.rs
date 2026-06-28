//! Postgres-backed database registry CRUD; seals/open credentials via SecretStore.

use egret_core::UserId;
use egret_metastore::Db;
use egret_secrets::SecretStore;
use uuid::Uuid;

use crate::error::DatabaseError;
use crate::model::{ConnOptions, Database, DatabaseInput, Engine};

#[derive(Clone)]
pub struct DatabaseRepo {
    db: Db,
    secrets: SecretStore,
}

#[derive(sqlx::FromRow)]
pub(crate) struct Row {
    id: Uuid,
    name: String,
    engine: String,
    host: String,
    port: i32,
    options: serde_json::Value,
    secret_id: Option<Uuid>,
    status: String,
}

impl Row {
    pub(crate) fn into_database(self) -> Result<Database, DatabaseError> {
        let engine = Engine::parse(&self.engine)
            .ok_or_else(|| DatabaseError::BadRequest(format!("unknown engine: {}", self.engine)))?;
        let options: ConnOptions = serde_json::from_value(self.options).unwrap_or_default();
        Ok(Database {
            id: self.id,
            name: self.name,
            engine,
            host: self.host,
            port: self.port,
            options,
            has_secret: self.secret_id.is_some(),
            status: self.status,
        })
    }
}

impl DatabaseRepo {
    pub fn new(db: Db, secrets: SecretStore) -> Self {
        Self { db, secrets }
    }

    /// Expose the secret store (used by routes to open a stored secret for testing).
    pub fn secrets(&self) -> &SecretStore {
        &self.secrets
    }

    /// Register a new database owned by `workspace_id`, sealing `secret` if provided.
    ///
    /// ADR-19 per-workspace model: the row is stamped with its owning workspace
    /// so workspace-scoped reads can filter it.
    pub async fn create_in_workspace(
        &self,
        workspace_id: Uuid,
        input: &DatabaseInput,
        created_by: Option<UserId>,
    ) -> Result<Database, DatabaseError> {
        let secret_id = match &input.secret {
            Some(s) if !s.is_empty() => Some(self.secrets.put("db_password", s).await?),
            _ => None,
        };
        let opts = serde_json::to_value(&input.options).unwrap_or(serde_json::json!({}));
        let row = sqlx::query_as::<_, Row>(
            "INSERT INTO databases (name, engine, host, port, options, secret_id, created_by, workspace_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             RETURNING id, name, engine, host, port, options, secret_id, status",
        )
        .bind(&input.name)
        .bind(input.engine.as_str())
        .bind(&input.host)
        .bind(input.port)
        .bind(opts)
        .bind(secret_id)
        .bind(created_by.map(|u| u.0))
        .bind(workspace_id)
        .fetch_one(&self.db.pool)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.is_unique_violation() => {
                DatabaseError::AlreadyExists(input.name.clone())
            }
            _ => DatabaseError::Db(e),
        });
        // If insert failed after sealing a secret, clean it up to avoid orphans.
        match row {
            Ok(r) => r.into_database(),
            Err(e) => {
                if let Some(sid) = secret_id {
                    if let Err(del) = self.secrets.delete(sid).await {
                        tracing::warn!(secret_id = %sid, error = %del,
                            "failed to clean up orphaned secret after insert failure");
                    }
                }
                Err(e)
            }
        }
    }

    pub async fn get(&self, id: Uuid) -> Result<Database, DatabaseError> {
        sqlx::query_as::<_, Row>(
            "SELECT id, name, engine, host, port, options, secret_id, status \
             FROM databases WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.db.pool)
        .await?
        .ok_or_else(|| DatabaseError::NotFound(id.to_string()))?
        .into_database()
    }

    /// Databases owned by a workspace (ordered by name).
    ///
    /// Ownership query (ADR-19 per-workspace model): rows are filtered by their
    /// own `workspace_id` column. Used by the workspace router and the query
    /// engine to scope visibility.
    pub async fn list_for_workspace(
        &self,
        workspace_id: Uuid,
    ) -> Result<Vec<Database>, DatabaseError> {
        let rows = sqlx::query_as::<_, Row>(
            "SELECT id, name, engine, host, port, options, secret_id, status \
             FROM databases WHERE workspace_id = $1 ORDER BY name",
        )
        .bind(workspace_id)
        .fetch_all(&self.db.pool)
        .await?;
        rows.into_iter().map(Row::into_database).collect()
    }

    /// Open the stored plaintext secret for a database (used by the connection tester).
    pub async fn open_secret(&self, id: Uuid) -> Result<Option<String>, DatabaseError> {
        let secret_id: Option<Uuid> =
            sqlx::query_scalar("SELECT secret_id FROM databases WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.db.pool)
                .await?
                .flatten();
        match secret_id {
            None => Ok(None),
            Some(sid) => Ok(Some(self.secrets.get(sid).await?)),
        }
    }

    /// Fetch a database only if it belongs to `workspace_id`.
    ///
    /// IDOR-safe: a row owned by another workspace (or absent) is reported as
    /// `NotFound`, never leaked.
    pub async fn get_in_workspace(
        &self,
        workspace_id: Uuid,
        id: Uuid,
    ) -> Result<Database, DatabaseError> {
        sqlx::query_as::<_, Row>(
            "SELECT id, name, engine, host, port, options, secret_id, status \
             FROM databases WHERE id = $1 AND workspace_id = $2",
        )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(&self.db.pool)
        .await?
        .ok_or_else(|| DatabaseError::NotFound(id.to_string()))?
        .into_database()
    }

    /// Update a database scoped to `workspace_id`.
    ///
    /// Ownership is checked first (404 before any mutation), then the same
    /// secret rotation/cleanup logic as [`update`](Self::update) runs, with every
    /// statement additionally constrained to the owning workspace.
    pub async fn update_in_workspace(
        &self,
        workspace_id: Uuid,
        id: Uuid,
        input: &DatabaseInput,
        _actor: Option<UserId>,
    ) -> Result<Database, DatabaseError> {
        self.get_in_workspace(workspace_id, id).await?; // 404 if not owned, before any mutation
        let old_secret_id: Option<Uuid> = sqlx::query_scalar(
            "SELECT secret_id FROM databases WHERE id = $1 AND workspace_id = $2",
        )
        .bind(id)
        .bind(workspace_id)
        .fetch_one(&self.db.pool)
        .await?;

        // Seal a new secret only when a non-empty plaintext is supplied.
        let sealed_new = match &input.secret {
            Some(s) if !s.is_empty() => Some(self.secrets.put("db_password", s).await?),
            _ => None,
        };
        // Keep the existing secret when none supplied.
        let new_secret_id = sealed_new.or(old_secret_id);
        let opts = serde_json::to_value(&input.options).unwrap_or(serde_json::json!({}));

        let res = sqlx::query(
            "UPDATE databases SET name=$2, engine=$3, host=$4, port=$5, options=$6, \
                 secret_id=$7, updated_at=now() WHERE id=$1 AND workspace_id=$8",
        )
        .bind(id)
        .bind(&input.name)
        .bind(input.engine.as_str())
        .bind(&input.host)
        .bind(input.port)
        .bind(opts)
        .bind(new_secret_id)
        .bind(workspace_id)
        .execute(&self.db.pool)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.is_unique_violation() => {
                DatabaseError::AlreadyExists(input.name.clone())
            }
            _ => DatabaseError::Db(e),
        });

        if let Err(e) = res {
            // Roll back a freshly-sealed secret so a failed update can't orphan it.
            if let Some(nid) = sealed_new {
                if let Err(del) = self.secrets.delete(nid).await {
                    tracing::warn!(secret_id = %nid, error = %del,
                        "failed to clean up orphaned secret after update failure");
                }
            }
            return Err(e);
        }

        // Rotation: delete the old secret only if we actually sealed a new one.
        if sealed_new.is_some() {
            if let Some(old) = old_secret_id {
                if let Err(del) = self.secrets.delete(old).await {
                    tracing::warn!(secret_id = %old, error = %del,
                        "failed to delete rotated-out secret");
                }
            }
        }
        self.get_in_workspace(workspace_id, id).await
    }

    /// Delete a database scoped to `workspace_id` (and its secret).
    ///
    /// Ownership is checked first (404 if not owned), then the row is deleted
    /// under the same workspace constraint and its sealed secret is cleaned up.
    pub async fn delete_in_workspace(
        &self,
        workspace_id: Uuid,
        id: Uuid,
    ) -> Result<(), DatabaseError> {
        self.get_in_workspace(workspace_id, id).await?; // 404 if not owned
        let secret_id: Option<Uuid> = sqlx::query_scalar(
            "SELECT secret_id FROM databases WHERE id = $1 AND workspace_id = $2",
        )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(&self.db.pool)
        .await?
        .flatten();
        sqlx::query("DELETE FROM databases WHERE id = $1 AND workspace_id = $2")
            .bind(id)
            .bind(workspace_id)
            .execute(&self.db.pool)
            .await?;
        if let Some(sid) = secret_id {
            if let Err(del) = self.secrets.delete(sid).await {
                tracing::warn!(secret_id = %sid, error = %del,
                    "failed to delete secret for removed database");
            }
        }
        Ok(())
    }

    /// Record the last connection-test result, scoped to the owning workspace
    /// (defense-in-depth: the status write cannot touch another workspace's row).
    pub async fn set_status(
        &self,
        workspace_id: Uuid,
        id: Uuid,
        status: &str,
    ) -> Result<(), DatabaseError> {
        sqlx::query(
            "UPDATE databases SET status=$3, updated_at=now() WHERE id=$1 AND workspace_id=$2",
        )
        .bind(id)
        .bind(workspace_id)
        .bind(status)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }
}
