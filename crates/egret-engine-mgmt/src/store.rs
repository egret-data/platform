//! Engine-mgmt store: CRUD for the `engine_mgmt.engines` / `engine_mgmt.engine_catalogs`
//! tables (this seam owns them). Runtime sqlx queries; schema-qualified.

use chrono::{DateTime, Utc};
use egret_metastore::Db;
use uuid::Uuid;

use crate::error::EngineMgmtError;

/// A provisioned query engine (control-plane, platform-global) wired to a
/// warehouse. `spec` holds the desired provisioning parameters; `status` tracks
/// the last-known lifecycle phase.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct Engine {
    pub id: Uuid,
    pub name: String,
    pub r#type: String,
    pub namespace: String,
    pub warehouse_id: Uuid,
    pub spec: serde_json::Value,
    pub status: String,
    pub authz_service_id: Option<i64>,
    pub created_at: DateTime<Utc>,
}

/// A catalog attached to a provisioned engine. Each row renders to one Trino
/// `<name>.properties` file; `kind` selects the connector translation and
/// `config` holds the typed per-kind parameters.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct EngineCatalog {
    pub id: Uuid,
    pub engine_id: Uuid,
    pub name: String,
    pub kind: String,
    pub config: serde_json::Value,
    pub secret_ref: Option<Uuid>,
    pub is_primary: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct EngineMgmtStore {
    db: Db,
}

impl EngineMgmtStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }
    fn pool(&self) -> &sqlx::PgPool {
        &self.db.pool
    }

    // engines ------------------------------------------------------------------

    pub async fn create_engine(
        &self,
        name: &str,
        r#type: &str,
        namespace: &str,
        warehouse_id: Uuid,
        spec: &serde_json::Value,
    ) -> Result<Engine, EngineMgmtError> {
        sqlx::query_as::<_, Engine>(
            "INSERT INTO engine_mgmt.engines (name, type, namespace, warehouse_id, spec) VALUES ($1,$2,$3,$4,$5) \
             RETURNING id, name, type, namespace, warehouse_id, spec, status, authz_service_id, created_at",
        )
        .bind(name)
        .bind(r#type)
        .bind(namespace)
        .bind(warehouse_id)
        .bind(spec)
        .fetch_one(self.pool())
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.constraint() == Some("engines_name_key") => {
                EngineMgmtError::AlreadyExists(name.to_string())
            }
            _ => EngineMgmtError::Db(e),
        })
    }

    pub async fn list_engines(&self) -> Result<Vec<Engine>, EngineMgmtError> {
        Ok(sqlx::query_as::<_, Engine>(
            "SELECT id, name, type, namespace, warehouse_id, spec, status, authz_service_id, created_at \
             FROM engine_mgmt.engines ORDER BY created_at",
        )
        .fetch_all(self.pool())
        .await?)
    }

    pub async fn get_engine(&self, id: Uuid) -> Result<Option<Engine>, EngineMgmtError> {
        Ok(sqlx::query_as::<_, Engine>(
            "SELECT id, name, type, namespace, warehouse_id, spec, status, authz_service_id, created_at \
             FROM engine_mgmt.engines WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await?)
    }

    pub async fn update_engine(
        &self,
        id: Uuid,
        spec: &serde_json::Value,
        status: &str,
    ) -> Result<(), EngineMgmtError> {
        sqlx::query("UPDATE engine_mgmt.engines SET spec = $2, status = $3 WHERE id = $1")
            .bind(id)
            .bind(spec)
            .bind(status)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn set_engine_status(&self, id: Uuid, status: &str) -> Result<(), EngineMgmtError> {
        sqlx::query("UPDATE engine_mgmt.engines SET status = $2 WHERE id = $1")
            .bind(id)
            .bind(status)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Bind (or detach with `None`) an engine to a Ranger policy profile.
    pub async fn set_engine_authz_service(
        &self,
        id: Uuid,
        authz_service_id: Option<i64>,
    ) -> Result<(), EngineMgmtError> {
        sqlx::query("UPDATE engine_mgmt.engines SET authz_service_id = $2 WHERE id = $1")
            .bind(id)
            .bind(authz_service_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn delete_engine(&self, id: Uuid) -> Result<(), EngineMgmtError> {
        sqlx::query("DELETE FROM engine_mgmt.engines WHERE id = $1")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    // engine catalogs ----------------------------------------------------------

    pub async fn create_engine_catalog(
        &self,
        engine_id: Uuid,
        name: &str,
        kind: &str,
        config: &serde_json::Value,
        secret_ref: Option<Uuid>,
        is_primary: bool,
    ) -> Result<EngineCatalog, EngineMgmtError> {
        sqlx::query_as::<_, EngineCatalog>(
            "INSERT INTO engine_mgmt.engine_catalogs (engine_id, name, kind, config, secret_ref, is_primary) \
             VALUES ($1,$2,$3,$4,$5,$6) \
             RETURNING id, engine_id, name, kind, config, secret_ref, is_primary, created_at",
        )
        .bind(engine_id)
        .bind(name)
        .bind(kind)
        .bind(config)
        .bind(secret_ref)
        .bind(is_primary)
        .fetch_one(self.pool())
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d)
                if d.constraint() == Some("engine_catalogs_engine_id_name_key") =>
            {
                EngineMgmtError::AlreadyExists(name.to_string())
            }
            _ => EngineMgmtError::Db(e),
        })
    }

    pub async fn list_engine_catalogs(
        &self,
        engine_id: Uuid,
    ) -> Result<Vec<EngineCatalog>, EngineMgmtError> {
        Ok(sqlx::query_as::<_, EngineCatalog>(
            "SELECT id, engine_id, name, kind, config, secret_ref, is_primary, created_at \
             FROM engine_mgmt.engine_catalogs WHERE engine_id = $1 ORDER BY is_primary DESC, created_at",
        )
        .bind(engine_id)
        .fetch_all(self.pool())
        .await?)
    }

    pub async fn get_engine_catalog(
        &self,
        id: Uuid,
    ) -> Result<Option<EngineCatalog>, EngineMgmtError> {
        Ok(sqlx::query_as::<_, EngineCatalog>(
            "SELECT id, engine_id, name, kind, config, secret_ref, is_primary, created_at \
             FROM engine_mgmt.engine_catalogs WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await?)
    }

    pub async fn update_engine_catalog(
        &self,
        id: Uuid,
        config: &serde_json::Value,
        secret_ref: Option<Uuid>,
    ) -> Result<Option<EngineCatalog>, EngineMgmtError> {
        Ok(sqlx::query_as::<_, EngineCatalog>(
            "UPDATE engine_mgmt.engine_catalogs SET config = $2, secret_ref = $3 WHERE id = $1 \
             RETURNING id, engine_id, name, kind, config, secret_ref, is_primary, created_at",
        )
        .bind(id)
        .bind(config)
        .bind(secret_ref)
        .fetch_optional(self.pool())
        .await?)
    }

    pub async fn delete_engine_catalog(&self, id: Uuid) -> Result<(), EngineMgmtError> {
        sqlx::query("DELETE FROM engine_mgmt.engine_catalogs WHERE id = $1")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(())
    }
}
