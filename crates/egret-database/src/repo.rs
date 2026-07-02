//! Postgres-backed database registry CRUD; seals/open credentials via SecretStore.

use egret_core::UserId;
use egret_metastore::Db;
use egret_secrets::{SecretStore, WorkspaceSecretStore};
use uuid::Uuid;

use crate::error::DatabaseError;
use crate::model::{ConnOptions, Database, DatabaseInput, Engine};

#[derive(Clone)]
pub struct DatabaseRepo {
    db: Db,
    secrets: SecretStore,
    workspace_secrets: WorkspaceSecretStore,
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
    pub fn new(db: Db, secrets: SecretStore, workspace_secrets: WorkspaceSecretStore) -> Self {
        Self {
            db,
            secrets,
            workspace_secrets,
        }
    }

    /// Expose the secret store (used by routes to open a stored secret for testing).
    pub fn secrets(&self) -> &SecretStore {
        &self.secrets
    }

    /// Expose the named workspace-secret store (Epic B: routes resolve a
    /// `new_secret`/`secret_id` to a named secret id before create/update).
    pub fn workspace_secrets(&self) -> &WorkspaceSecretStore {
        &self.workspace_secrets
    }

    /// Register a new database owned by `workspace_id`, binding the already-resolved
    /// named secret id (Epic B: the route resolves `new_secret`/`secret_id` to a
    /// `workspace_secrets` id; the repo just binds it — no sealing here).
    ///
    /// ADR-19 per-workspace model: the row is stamped with its owning workspace
    /// so workspace-scoped reads can filter it.
    pub async fn create_in_workspace(
        &self,
        workspace_id: Uuid,
        input: &DatabaseInput,
        secret_id: Option<Uuid>,
        created_by: Option<UserId>,
    ) -> Result<Database, DatabaseError> {
        let opts = serde_json::to_value(&input.options).unwrap_or(serde_json::json!({}));
        sqlx::query_as::<_, Row>(
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
        })?
        .into_database()
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

    /// Open the stored plaintext secret for a database (used by the connection
    /// tester). The named secret (Epic B) is resolved workspace-scoped: the
    /// owning `workspace_id` is read from the row itself, so the engine job and
    /// the test route keep the same `open_secret(id)` signature. NULL `secret_id`
    /// (or no row) → `Ok(None)`.
    pub async fn open_secret(&self, id: Uuid) -> Result<Option<String>, DatabaseError> {
        let row: Option<(Option<Uuid>, Uuid)> =
            sqlx::query_as("SELECT secret_id, workspace_id FROM databases WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.db.pool)
                .await?;
        match row {
            Some((Some(secret_id), workspace_id)) => Ok(Some(
                self.workspace_secrets
                    .resolve_plaintext(Some(workspace_id), secret_id)
                    .await?,
            )),
            _ => Ok(None),
        }
    }

    /// The current `secret_id` bound to a database owned by `workspace_id`
    /// (used by the update route to keep the existing named secret when neither
    /// `new_secret` nor `secret_id` is supplied). `NotFound` if the row is absent
    /// or owned by another workspace (IDOR closed); `Ok(None)` if it has no secret.
    pub async fn current_secret_id(
        &self,
        workspace_id: Uuid,
        id: Uuid,
    ) -> Result<Option<Uuid>, DatabaseError> {
        let row: Option<Option<Uuid>> = sqlx::query_scalar(
            "SELECT secret_id FROM databases WHERE id = $1 AND workspace_id = $2",
        )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(&self.db.pool)
        .await?;
        row.ok_or_else(|| DatabaseError::NotFound(id.to_string()))
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

    /// Update a database scoped to `workspace_id`, binding the already-resolved
    /// named secret id (Epic B: the route resolves `new_secret`/`secret_id`,
    /// defaulting to the existing `secret_ref` for keep-current; the repo just
    /// binds the resulting id — no sealing or rotation cleanup here, since named
    /// secrets are shared and owned by `WorkspaceSecretStore`).
    ///
    /// Ownership is checked first (404 before any mutation), and every statement
    /// is constrained to the owning workspace.
    pub async fn update_in_workspace(
        &self,
        workspace_id: Uuid,
        id: Uuid,
        input: &DatabaseInput,
        secret_id: Option<Uuid>,
        _actor: Option<UserId>,
    ) -> Result<Database, DatabaseError> {
        self.get_in_workspace(workspace_id, id).await?; // 404 if not owned, before any mutation
        let opts = serde_json::to_value(&input.options).unwrap_or(serde_json::json!({}));

        sqlx::query(
            "UPDATE databases SET name=$2, engine=$3, host=$4, port=$5, options=$6, \
                 secret_id=$7, updated_at=now() WHERE id=$1 AND workspace_id=$8",
        )
        .bind(id)
        .bind(&input.name)
        .bind(input.engine.as_str())
        .bind(&input.host)
        .bind(input.port)
        .bind(opts)
        .bind(secret_id)
        .bind(workspace_id)
        .execute(&self.db.pool)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.is_unique_violation() => {
                DatabaseError::AlreadyExists(input.name.clone())
            }
            _ => DatabaseError::Db(e),
        })?;

        self.get_in_workspace(workspace_id, id).await
    }

    /// Delete a database scoped to `workspace_id`.
    ///
    /// Ownership is checked first (404 if not owned), then the row is deleted
    /// under the same workspace constraint. The referenced NAMED secret (Epic B)
    /// is left intact — it is shared and owned by `WorkspaceSecretStore`; removing
    /// a connection only drops its reference (decrementing the secret's
    /// `used_by`), it does not delete the secret.
    pub async fn delete_in_workspace(
        &self,
        workspace_id: Uuid,
        id: Uuid,
    ) -> Result<(), DatabaseError> {
        self.get_in_workspace(workspace_id, id).await?; // 404 if not owned
        sqlx::query("DELETE FROM databases WHERE id = $1 AND workspace_id = $2")
            .bind(id)
            .bind(workspace_id)
            .execute(&self.db.pool)
            .await?;
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
