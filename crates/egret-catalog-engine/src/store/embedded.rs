//! Embedded-catalog RBAC store: warehouses (per workspace/domain), catalog-roles
//! (keyed by consumer-type), grants, and per-consumer Catalog Clients. egret owns
//! this store; the mini-IdP's [`ClientVerifier`] + the lakekeeper Authorizer (EB-3)
//! read from it.

use crate::store::CatalogError;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use base64::Engine as _;
use chrono::{DateTime, Utc};
use rand::Rng as _;
use uuid::Uuid;

use super::handle::CatalogStore;

// ── warehouses ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Warehouse {
    pub id: Uuid,
    pub name: String,
    pub workspace_id: Uuid,
    pub storage_conn_id: Uuid,
    pub lakekeeper_warehouse_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

// ── roles + grants ───────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct CatalogRole {
    pub id: Uuid,
    pub warehouse_id: Uuid,
    pub name: String,
    pub consumer_type: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug)]
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

#[derive(Clone, Debug)]
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

// ── FromRow impls (hand-written, sqlx08) ─────────────────────────────────────
// These are hand-written rather than `#[derive(sqlx08::FromRow)]` because sqlx's
// derive emits a hard-coded `::sqlx` path in its generated code. In this crate
// the bare `sqlx` name is the sqlx-0.9 lakekeeper surface, so the derived impl
// would target the wrong (0.9) `FromRow`/`Row` traits while the store's queries
// run through the 0.8 alias `sqlx08`. Column names/types are unchanged, so these
// are behaviourally identical to the former derive.

impl sqlx08::FromRow<'_, sqlx08::postgres::PgRow> for Warehouse {
    fn from_row(row: &sqlx08::postgres::PgRow) -> sqlx08::Result<Self> {
        use sqlx08::Row as _;
        Ok(Self {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            workspace_id: row.try_get("workspace_id")?,
            storage_conn_id: row.try_get("storage_conn_id")?,
            lakekeeper_warehouse_id: row.try_get("lakekeeper_warehouse_id")?,
            created_at: row.try_get("created_at")?,
        })
    }
}

impl sqlx08::FromRow<'_, sqlx08::postgres::PgRow> for CatalogRole {
    fn from_row(row: &sqlx08::postgres::PgRow) -> sqlx08::Result<Self> {
        use sqlx08::Row as _;
        Ok(Self {
            id: row.try_get("id")?,
            warehouse_id: row.try_get("warehouse_id")?,
            name: row.try_get("name")?,
            consumer_type: row.try_get("consumer_type")?,
            description: row.try_get("description")?,
        })
    }
}

impl sqlx08::FromRow<'_, sqlx08::postgres::PgRow> for CatalogGrant {
    fn from_row(row: &sqlx08::postgres::PgRow) -> sqlx08::Result<Self> {
        use sqlx08::Row as _;
        Ok(Self {
            id: row.try_get("id")?,
            catalog_role_id: row.try_get("catalog_role_id")?,
            securable_type: row.try_get("securable_type")?,
            namespace: row.try_get("namespace")?,
            table_name: row.try_get("table_name")?,
            privilege: row.try_get("privilege")?,
        })
    }
}

impl sqlx08::FromRow<'_, sqlx08::postgres::PgRow> for CatalogClient {
    fn from_row(row: &sqlx08::postgres::PgRow) -> sqlx08::Result<Self> {
        use sqlx08::Row as _;
        Ok(Self {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            consumer_type: row.try_get("consumer_type")?,
            workspace_id: row.try_get("workspace_id")?,
            warehouse_id: row.try_get("warehouse_id")?,
            catalog_role_id: row.try_get("catalog_role_id")?,
            client_id: row.try_get("client_id")?,
            created_at: row.try_get("created_at")?,
            last_used_at: row.try_get("last_used_at")?,
        })
    }
}

impl CatalogStore {
    // warehouses ---------------------------------------------------------------

    pub async fn create_warehouse(
        &self,
        name: &str,
        workspace_id: Uuid,
        storage_conn_id: Uuid,
    ) -> Result<Warehouse, CatalogError> {
        sqlx08::query_as::<_, Warehouse>(
            "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ($1,$2,$3) \
             RETURNING id, name, workspace_id, storage_conn_id, lakekeeper_warehouse_id, created_at",
        )
        .bind(name)
        .bind(workspace_id)
        .bind(storage_conn_id)
        .fetch_one(self.pool())
        .await
        .map_err(|e| match &e {
            sqlx08::Error::Database(d) if d.constraint() == Some("warehouses_workspace_id_name_key") => {
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
        sqlx08::query("UPDATE catalog.warehouses SET lakekeeper_warehouse_id = $2 WHERE id = $1")
            .bind(id)
            .bind(lk_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn list_warehouses(&self) -> Result<Vec<Warehouse>, CatalogError> {
        Ok(sqlx08::query_as::<_, Warehouse>(
            "SELECT id, name, workspace_id, storage_conn_id, lakekeeper_warehouse_id, created_at \
             FROM catalog.warehouses ORDER BY name",
        )
        .fetch_all(self.pool())
        .await?)
    }

    pub async fn get_warehouse(&self, id: Uuid) -> Result<Option<Warehouse>, CatalogError> {
        Ok(sqlx08::query_as::<_, Warehouse>(
            "SELECT id, name, workspace_id, storage_conn_id, lakekeeper_warehouse_id, created_at \
             FROM catalog.warehouses WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await?)
    }

    pub async fn get_warehouse_by_name(
        &self,
        name: &str,
    ) -> Result<Option<Warehouse>, CatalogError> {
        Ok(sqlx08::query_as::<_, Warehouse>(
            "SELECT id, name, workspace_id, storage_conn_id, lakekeeper_warehouse_id, created_at \
             FROM catalog.warehouses WHERE name = $1",
        )
        .bind(name)
        .fetch_optional(self.pool())
        .await?)
    }

    pub async fn delete_warehouse(&self, id: Uuid) -> Result<(), CatalogError> {
        sqlx08::query("DELETE FROM catalog.warehouses WHERE id = $1")
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
        sqlx08::query_as::<_, CatalogRole>(
            "INSERT INTO catalog.catalog_roles (warehouse_id, name, consumer_type, description) VALUES ($1,$2,$3,$4) \
             RETURNING id, warehouse_id, name, consumer_type, description",
        )
        .bind(warehouse_id)
        .bind(name)
        .bind(consumer_type)
        .bind(description)
        .fetch_one(self.pool())
        .await
        .map_err(|e| match &e {
            sqlx08::Error::Database(d)
                if d.constraint() == Some("catalog_roles_warehouse_id_name_key") =>
            {
                CatalogError::AlreadyExists(name.to_string())
            }
            _ => CatalogError::Db(e),
        })
    }

    pub async fn list_roles(&self, warehouse_id: Uuid) -> Result<Vec<CatalogRole>, CatalogError> {
        Ok(sqlx08::query_as::<_, CatalogRole>(
            "SELECT id, warehouse_id, name, consumer_type, description FROM catalog.catalog_roles \
             WHERE warehouse_id = $1 ORDER BY name",
        )
        .bind(warehouse_id)
        .fetch_all(self.pool())
        .await?)
    }

    /// Delete a role. Its grants cascade; blocked by FK if any Catalog Client
    /// still references it (surfaced as `CatalogError::InUse`).
    pub async fn delete_role(&self, role_id: Uuid) -> Result<(), CatalogError> {
        sqlx08::query("DELETE FROM catalog.catalog_roles WHERE id = $1")
            .bind(role_id)
            .execute(self.pool())
            .await
            .map_err(|e| match &e {
                sqlx08::Error::Database(d) if d.is_foreign_key_violation() => CatalogError::InUse,
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
        let exists: Option<Uuid> = sqlx08::query_scalar(
            "SELECT id FROM catalog.catalog_grants \
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
        Ok(sqlx08::query_as::<_, CatalogGrant>(
            "INSERT INTO catalog.catalog_grants (catalog_role_id, securable_type, namespace, table_name, privilege) \
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
        Ok(sqlx08::query_as::<_, CatalogGrant>(
            "SELECT id, catalog_role_id, securable_type, namespace, table_name, privilege \
             FROM catalog.catalog_grants WHERE catalog_role_id = $1",
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
        Ok(sqlx08::query_as::<_, CatalogGrant>(
            "SELECT g.id, g.catalog_role_id, g.securable_type, g.namespace, g.table_name, g.privilege \
             FROM catalog.catalog_grants g \
             JOIN catalog.catalog_roles r ON r.id = g.catalog_role_id \
             JOIN catalog.warehouses w ON w.id = r.warehouse_id \
             WHERE w.name = $1 AND r.name = $2",
        )
        .bind(warehouse_name)
        .bind(role_name)
        .fetch_all(self.pool())
        .await?)
    }

    pub async fn revoke_grant(&self, grant_id: Uuid) -> Result<(), CatalogError> {
        sqlx08::query("DELETE FROM catalog.catalog_grants WHERE id = $1")
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
        let id: Uuid = sqlx08::query_scalar(
            "INSERT INTO catalog.catalog_clients \
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
        Ok(sqlx08::query_as::<_, CatalogClient>(
            "SELECT id, name, consumer_type, workspace_id, warehouse_id, catalog_role_id, \
                    client_id, created_at, last_used_at \
             FROM catalog.catalog_clients WHERE warehouse_id = $1 ORDER BY name",
        )
        .bind(warehouse_id)
        .fetch_all(self.pool())
        .await?)
    }

    pub async fn revoke_client(&self, id: Uuid) -> Result<(), CatalogError> {
        sqlx08::query("DELETE FROM catalog.catalog_clients WHERE id = $1")
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
        let row: Option<(Uuid, String, String, String)> = sqlx08::query_as(
            "SELECT c.id, c.client_secret_hash, w.name, r.name \
             FROM catalog.catalog_clients c \
             JOIN catalog.warehouses w ON w.id = c.warehouse_id \
             JOIN catalog.catalog_roles r ON r.id = c.catalog_role_id \
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
        let _ =
            sqlx08::query("UPDATE catalog.catalog_clients SET last_used_at = now() WHERE id = $1")
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
