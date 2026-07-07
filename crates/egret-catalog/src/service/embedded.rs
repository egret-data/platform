//! Embedded-catalog RBAC store: warehouses (per workspace/domain), catalog-roles
//! (keyed by consumer-type), grants, and per-consumer Catalog Clients. egret owns
//! this store; the mini-IdP's [`ClientVerifier`] + the lakekeeper Authorizer (EB-3)
//! read from it.

use crate::CatalogError;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use base64::Engine as _;
use chrono::{DateTime, Utc};
use rand::Rng as _;
use uuid::Uuid;

use super::store::CatalogStore;

// ── warehouses ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct Warehouse {
    pub id: Uuid,
    pub name: String,
    pub workspace_id: Uuid,
    pub storage_conn_id: Uuid,
    pub lakekeeper_warehouse_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

// ── engines ──────────────────────────────────────────────────────────────────

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

// ── roles + grants ───────────────────────────────────────────────────────────

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct CatalogRole {
    pub id: Uuid,
    pub warehouse_id: Uuid,
    pub name: String,
    pub consumer_type: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct CatalogGrant {
    pub id: Uuid,
    pub catalog_role_id: Uuid,
    pub securable_type: String,
    pub namespace: Option<Vec<String>>,
    pub table_name: Option<String>,
    pub privilege: String,
}

pub struct GrantInput {
    pub securable_type: String,
    pub namespace: Option<Vec<String>>,
    pub table_name: Option<String>,
    pub privilege: String,
}

// ── clients ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct CatalogClient {
    pub id: Uuid,
    pub name: String,
    pub consumer_type: String,
    pub workspace_id: Uuid,
    pub warehouse_id: Uuid,
    pub catalog_role_id: Uuid,
    pub client_id: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

pub struct NewClient {
    pub name: String,
    pub consumer_type: String,
    pub workspace_id: Uuid,
    pub warehouse_id: Uuid,
    pub catalog_role_id: Uuid,
}

/// The one-time create result: the generated `client_id` + plaintext `client_secret`.
pub struct CreatedClient {
    pub id: Uuid,
    pub client_id: String,
    /// Shown ONCE; only the argon2 hash is persisted.
    pub client_secret: String,
}

impl CatalogStore {
    // warehouses ---------------------------------------------------------------

    pub async fn create_warehouse(
        &self,
        name: &str,
        workspace_id: Uuid,
        storage_conn_id: Uuid,
    ) -> Result<Warehouse, CatalogError> {
        sqlx::query_as::<_, Warehouse>(
            "INSERT INTO warehouses (name, workspace_id, storage_conn_id) VALUES ($1,$2,$3) \
             RETURNING id, name, workspace_id, storage_conn_id, lakekeeper_warehouse_id, created_at",
        )
        .bind(name)
        .bind(workspace_id)
        .bind(storage_conn_id)
        .fetch_one(self.pool())
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.constraint() == Some("warehouses_workspace_id_name_key") => {
                CatalogError::AlreadyExists(name.to_string())
            }
            _ => CatalogError::Db(e),
        })
    }

    pub async fn set_lakekeeper_warehouse_id(
        &self,
        id: Uuid,
        lk_id: &str,
    ) -> Result<(), CatalogError> {
        sqlx::query("UPDATE warehouses SET lakekeeper_warehouse_id = $2 WHERE id = $1")
            .bind(id)
            .bind(lk_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn list_warehouses(&self) -> Result<Vec<Warehouse>, CatalogError> {
        Ok(sqlx::query_as::<_, Warehouse>(
            "SELECT id, name, workspace_id, storage_conn_id, lakekeeper_warehouse_id, created_at \
             FROM warehouses ORDER BY name",
        )
        .fetch_all(self.pool())
        .await?)
    }

    pub async fn get_warehouse(&self, id: Uuid) -> Result<Option<Warehouse>, CatalogError> {
        Ok(sqlx::query_as::<_, Warehouse>(
            "SELECT id, name, workspace_id, storage_conn_id, lakekeeper_warehouse_id, created_at \
             FROM warehouses WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await?)
    }

    pub async fn get_warehouse_by_name(
        &self,
        name: &str,
    ) -> Result<Option<Warehouse>, CatalogError> {
        Ok(sqlx::query_as::<_, Warehouse>(
            "SELECT id, name, workspace_id, storage_conn_id, lakekeeper_warehouse_id, created_at \
             FROM warehouses WHERE name = $1",
        )
        .bind(name)
        .fetch_optional(self.pool())
        .await?)
    }

    pub async fn delete_warehouse(&self, id: Uuid) -> Result<(), CatalogError> {
        sqlx::query("DELETE FROM warehouses WHERE id = $1")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    // engines ------------------------------------------------------------------

    pub async fn create_engine(
        &self,
        name: &str,
        r#type: &str,
        namespace: &str,
        warehouse_id: Uuid,
        spec: &serde_json::Value,
    ) -> Result<Engine, CatalogError> {
        sqlx::query_as::<_, Engine>(
            "INSERT INTO engines (name, type, namespace, warehouse_id, spec) VALUES ($1,$2,$3,$4,$5) \
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
                CatalogError::AlreadyExists(name.to_string())
            }
            _ => CatalogError::Db(e),
        })
    }

    pub async fn list_engines(&self) -> Result<Vec<Engine>, CatalogError> {
        Ok(sqlx::query_as::<_, Engine>(
            "SELECT id, name, type, namespace, warehouse_id, spec, status, authz_service_id, created_at \
             FROM engines ORDER BY created_at",
        )
        .fetch_all(self.pool())
        .await?)
    }

    pub async fn get_engine(&self, id: Uuid) -> Result<Option<Engine>, CatalogError> {
        Ok(sqlx::query_as::<_, Engine>(
            "SELECT id, name, type, namespace, warehouse_id, spec, status, authz_service_id, created_at \
             FROM engines WHERE id = $1",
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
    ) -> Result<(), CatalogError> {
        sqlx::query("UPDATE engines SET spec = $2, status = $3 WHERE id = $1")
            .bind(id)
            .bind(spec)
            .bind(status)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn set_engine_status(&self, id: Uuid, status: &str) -> Result<(), CatalogError> {
        sqlx::query("UPDATE engines SET status = $2 WHERE id = $1")
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
    ) -> Result<(), CatalogError> {
        sqlx::query("UPDATE engines SET authz_service_id = $2 WHERE id = $1")
            .bind(id)
            .bind(authz_service_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn delete_engine(&self, id: Uuid) -> Result<(), CatalogError> {
        sqlx::query("DELETE FROM engines WHERE id = $1")
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
    ) -> Result<EngineCatalog, CatalogError> {
        sqlx::query_as::<_, EngineCatalog>(
            "INSERT INTO engine_catalogs (engine_id, name, kind, config, secret_ref, is_primary) \
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
                CatalogError::AlreadyExists(name.to_string())
            }
            _ => CatalogError::Db(e),
        })
    }

    pub async fn list_engine_catalogs(
        &self,
        engine_id: Uuid,
    ) -> Result<Vec<EngineCatalog>, CatalogError> {
        Ok(sqlx::query_as::<_, EngineCatalog>(
            "SELECT id, engine_id, name, kind, config, secret_ref, is_primary, created_at \
             FROM engine_catalogs WHERE engine_id = $1 ORDER BY is_primary DESC, created_at",
        )
        .bind(engine_id)
        .fetch_all(self.pool())
        .await?)
    }

    pub async fn get_engine_catalog(
        &self,
        id: Uuid,
    ) -> Result<Option<EngineCatalog>, CatalogError> {
        Ok(sqlx::query_as::<_, EngineCatalog>(
            "SELECT id, engine_id, name, kind, config, secret_ref, is_primary, created_at \
             FROM engine_catalogs WHERE id = $1",
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
    ) -> Result<Option<EngineCatalog>, CatalogError> {
        Ok(sqlx::query_as::<_, EngineCatalog>(
            "UPDATE engine_catalogs SET config = $2, secret_ref = $3 WHERE id = $1 \
             RETURNING id, engine_id, name, kind, config, secret_ref, is_primary, created_at",
        )
        .bind(id)
        .bind(config)
        .bind(secret_ref)
        .fetch_optional(self.pool())
        .await?)
    }

    pub async fn delete_engine_catalog(&self, id: Uuid) -> Result<(), CatalogError> {
        sqlx::query("DELETE FROM engine_catalogs WHERE id = $1")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    // roles + grants -----------------------------------------------------------

    pub async fn create_role(
        &self,
        warehouse_id: Uuid,
        name: &str,
        consumer_type: &str,
        description: Option<&str>,
    ) -> Result<CatalogRole, CatalogError> {
        sqlx::query_as::<_, CatalogRole>(
            "INSERT INTO catalog_roles (warehouse_id, name, consumer_type, description) VALUES ($1,$2,$3,$4) \
             RETURNING id, warehouse_id, name, consumer_type, description",
        )
        .bind(warehouse_id)
        .bind(name)
        .bind(consumer_type)
        .bind(description)
        .fetch_one(self.pool())
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d)
                if d.constraint() == Some("catalog_roles_warehouse_id_name_key") =>
            {
                CatalogError::AlreadyExists(name.to_string())
            }
            _ => CatalogError::Db(e),
        })
    }

    pub async fn list_roles(&self, warehouse_id: Uuid) -> Result<Vec<CatalogRole>, CatalogError> {
        Ok(sqlx::query_as::<_, CatalogRole>(
            "SELECT id, warehouse_id, name, consumer_type, description FROM catalog_roles \
             WHERE warehouse_id = $1 ORDER BY name",
        )
        .bind(warehouse_id)
        .fetch_all(self.pool())
        .await?)
    }

    /// Delete a role. Its grants cascade; blocked by FK if any Catalog Client
    /// still references it (surfaced as `CatalogError::InUse`).
    pub async fn delete_role(&self, role_id: Uuid) -> Result<(), CatalogError> {
        sqlx::query("DELETE FROM catalog_roles WHERE id = $1")
            .bind(role_id)
            .execute(self.pool())
            .await
            .map_err(|e| match &e {
                sqlx::Error::Database(d) if d.is_foreign_key_violation() => CatalogError::InUse,
                _ => CatalogError::Db(e),
            })?;
        Ok(())
    }

    pub async fn add_grant(
        &self,
        catalog_role_id: Uuid,
        g: &GrantInput,
    ) -> Result<CatalogGrant, CatalogError> {
        // Reject a duplicate of an identical grant (same securable + privilege).
        // NULL namespace/table are compared with IS NOT DISTINCT FROM so
        // catalog-level duplicates are caught too.
        let exists: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM catalog_grants \
             WHERE catalog_role_id = $1 AND securable_type = $2 \
               AND namespace IS NOT DISTINCT FROM $3 AND table_name IS NOT DISTINCT FROM $4 \
               AND privilege = $5",
        )
        .bind(catalog_role_id)
        .bind(&g.securable_type)
        .bind(g.namespace.as_deref())
        .bind(g.table_name.as_deref())
        .bind(&g.privilege)
        .fetch_optional(self.pool())
        .await?;
        if exists.is_some() {
            return Err(CatalogError::AlreadyExists("grant".to_string()));
        }
        Ok(sqlx::query_as::<_, CatalogGrant>(
            "INSERT INTO catalog_grants (catalog_role_id, securable_type, namespace, table_name, privilege) \
             VALUES ($1,$2,$3,$4,$5) \
             RETURNING id, catalog_role_id, securable_type, namespace, table_name, privilege",
        )
        .bind(catalog_role_id)
        .bind(&g.securable_type)
        .bind(g.namespace.as_deref())
        .bind(g.table_name.as_deref())
        .bind(&g.privilege)
        .fetch_one(self.pool())
        .await?)
    }

    pub async fn list_grants(
        &self,
        catalog_role_id: Uuid,
    ) -> Result<Vec<CatalogGrant>, CatalogError> {
        Ok(sqlx::query_as::<_, CatalogGrant>(
            "SELECT id, catalog_role_id, securable_type, namespace, table_name, privilege \
             FROM catalog_grants WHERE catalog_role_id = $1",
        )
        .bind(catalog_role_id)
        .fetch_all(self.pool())
        .await?)
    }

    /// All grants for a role identified by (warehouse name, role name) — the
    /// lookup the Authorizer uses from a JWT's `warehouse`+`roles` claims (EB-3).
    pub async fn grants_for_role(
        &self,
        warehouse_name: &str,
        role_name: &str,
    ) -> Result<Vec<CatalogGrant>, CatalogError> {
        Ok(sqlx::query_as::<_, CatalogGrant>(
            "SELECT g.id, g.catalog_role_id, g.securable_type, g.namespace, g.table_name, g.privilege \
             FROM catalog_grants g \
             JOIN catalog_roles r ON r.id = g.catalog_role_id \
             JOIN warehouses w ON w.id = r.warehouse_id \
             WHERE w.name = $1 AND r.name = $2",
        )
        .bind(warehouse_name)
        .bind(role_name)
        .fetch_all(self.pool())
        .await?)
    }

    pub async fn revoke_grant(&self, grant_id: Uuid) -> Result<(), CatalogError> {
        sqlx::query("DELETE FROM catalog_grants WHERE id = $1")
            .bind(grant_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    // clients ------------------------------------------------------------------

    /// Create a Catalog Client: generate a `client_id` + secret (argon2-hashed at
    /// rest), returning the plaintext secret ONCE.
    pub async fn create_client(&self, c: &NewClient) -> Result<CreatedClient, CatalogError> {
        // Generate all random material in a block so the `!Send` ThreadRng is
        // dropped BEFORE the sqlx await (otherwise the future is !Send and can't
        // be used as an axum handler).
        let (client_id, client_secret) = {
            let mut rng = rand::thread_rng();
            let client_id = format!("egret_{}", hex::encode(rng.gen::<[u8; 8]>()));
            let secret_bytes: [u8; 32] = rng.gen();
            (
                client_id,
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret_bytes),
            )
        };
        let salt = SaltString::generate(&mut OsRng);
        let hash = Argon2::default()
            .hash_password(client_secret.as_bytes(), &salt)
            .map_err(|e| CatalogError::BadRequest(format!("hash: {e}")))?
            .to_string();
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO catalog_clients \
               (name, consumer_type, workspace_id, warehouse_id, catalog_role_id, client_id, client_secret_hash) \
             VALUES ($1,$2,$3,$4,$5,$6,$7) RETURNING id",
        )
        .bind(&c.name)
        .bind(&c.consumer_type)
        .bind(c.workspace_id)
        .bind(c.warehouse_id)
        .bind(c.catalog_role_id)
        .bind(&client_id)
        .bind(&hash)
        .fetch_one(self.pool())
        .await?;
        Ok(CreatedClient {
            id,
            client_id,
            client_secret,
        })
    }

    pub async fn list_clients(
        &self,
        warehouse_id: Uuid,
    ) -> Result<Vec<CatalogClient>, CatalogError> {
        Ok(sqlx::query_as::<_, CatalogClient>(
            "SELECT id, name, consumer_type, workspace_id, warehouse_id, catalog_role_id, \
                    client_id, created_at, last_used_at \
             FROM catalog_clients WHERE warehouse_id = $1 ORDER BY name",
        )
        .bind(warehouse_id)
        .fetch_all(self.pool())
        .await?)
    }

    pub async fn revoke_client(&self, id: Uuid) -> Result<(), CatalogError> {
        sqlx::query("DELETE FROM catalog_clients WHERE id = $1")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Verify a client's secret; on success return its warehouse name + role name
    /// (for the JWT claims), and touch `last_used_at`.
    pub async fn verify_client(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> Result<Option<(String, String)>, CatalogError> {
        let row: Option<(Uuid, String, String, String)> = sqlx::query_as(
            "SELECT c.id, c.client_secret_hash, w.name, r.name \
             FROM catalog_clients c \
             JOIN warehouses w ON w.id = c.warehouse_id \
             JOIN catalog_roles r ON r.id = c.catalog_role_id \
             WHERE c.client_id = $1",
        )
        .bind(client_id)
        .fetch_optional(self.pool())
        .await?;
        let Some((id, hash, warehouse, role)) = row else {
            return Ok(None);
        };
        let parsed = PasswordHash::new(&hash)
            .map_err(|e| CatalogError::BadRequest(format!("hash parse: {e}")))?;
        if Argon2::default()
            .verify_password(client_secret.as_bytes(), &parsed)
            .is_err()
        {
            return Ok(None);
        }
        let _ = sqlx::query("UPDATE catalog_clients SET last_used_at = now() WHERE id = $1")
            .bind(id)
            .execute(self.pool())
            .await;
        Ok(Some((warehouse, role)))
    }
}

/// The mini-IdP's client verifier, backed by the Catalog Client registry.
#[async_trait::async_trait]
impl egret_machine_idp::ClientVerifier for CatalogStore {
    async fn verify(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> Result<Option<egret_machine_idp::VerifiedClient>, egret_machine_idp::IdpError> {
        match self
            .verify_client(client_id, client_secret)
            .await
            .map_err(|e| egret_machine_idp::IdpError::Db(e.to_string()))?
        {
            Some((warehouse, catalog_role)) => Ok(Some(egret_machine_idp::VerifiedClient {
                client_id: client_id.to_string(),
                warehouse,
                catalog_role,
            })),
            None => Ok(None),
        }
    }
}
