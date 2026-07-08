//! `CatalogPort` (contract crate) implemented over the concrete `CatalogStore`.
//! The in-process wiring; at a real split an HTTP client implements the same
//! trait. Converts the `sqlx::FromRow` row structs -> pure-serde leaf DTOs and
//! `CatalogError -> CatalogApiError` at the seam boundary so no sqlx type escapes
//! the contract.
//!
//! The inherent `CatalogStore` methods (same names) shadow these trait methods,
//! so `self.<method>()` below resolves to the concrete implementation — exactly
//! as `egret-policy`'s `port_impl` relies on.
use crate::store::embedded as concrete;
use crate::store::CatalogError;
use crate::store::CatalogStore;
use egret_catalog_api::{
    CatalogApiError, CatalogClient, CatalogGrant, CatalogPort, CatalogRole, CreatedClient,
    GrantInput, NewClient, Warehouse,
};
use uuid::Uuid;

// ── row struct -> leaf DTO ────────────────────────────────────────────────────

impl From<concrete::Warehouse> for Warehouse {
    fn from(w: concrete::Warehouse) -> Self {
        Warehouse {
            id: w.id,
            name: w.name,
            workspace_id: w.workspace_id,
            storage_conn_id: w.storage_conn_id,
            lakekeeper_warehouse_id: w.lakekeeper_warehouse_id,
            created_at: w.created_at,
        }
    }
}

impl From<concrete::CatalogRole> for CatalogRole {
    fn from(r: concrete::CatalogRole) -> Self {
        CatalogRole {
            id: r.id,
            warehouse_id: r.warehouse_id,
            name: r.name,
            consumer_type: r.consumer_type,
            description: r.description,
        }
    }
}

impl From<concrete::CatalogGrant> for CatalogGrant {
    fn from(g: concrete::CatalogGrant) -> Self {
        CatalogGrant {
            id: g.id,
            catalog_role_id: g.catalog_role_id,
            securable_type: g.securable_type,
            namespace: g.namespace,
            table_name: g.table_name,
            privilege: g.privilege,
        }
    }
}

impl From<concrete::CatalogClient> for CatalogClient {
    fn from(c: concrete::CatalogClient) -> Self {
        CatalogClient {
            id: c.id,
            name: c.name,
            consumer_type: c.consumer_type,
            workspace_id: c.workspace_id,
            warehouse_id: c.warehouse_id,
            catalog_role_id: c.catalog_role_id,
            client_id: c.client_id,
            created_at: c.created_at,
            last_used_at: c.last_used_at,
        }
    }
}

impl From<concrete::CreatedClient> for CreatedClient {
    fn from(c: concrete::CreatedClient) -> Self {
        CreatedClient {
            id: c.id,
            client_id: c.client_id,
            client_secret: c.client_secret,
        }
    }
}

// ── leaf input DTO -> concrete input ──────────────────────────────────────────

impl From<GrantInput> for concrete::GrantInput {
    fn from(g: GrantInput) -> Self {
        concrete::GrantInput {
            securable_type: g.securable_type,
            namespace: g.namespace,
            table_name: g.table_name,
            privilege: g.privilege,
        }
    }
}

impl From<NewClient> for concrete::NewClient {
    fn from(c: NewClient) -> Self {
        concrete::NewClient {
            name: c.name,
            consumer_type: c.consumer_type,
            workspace_id: c.workspace_id,
            warehouse_id: c.warehouse_id,
            catalog_role_id: c.catalog_role_id,
        }
    }
}

// ── error -> transport-neutral error ──────────────────────────────────────────

impl From<CatalogError> for CatalogApiError {
    fn from(e: CatalogError) -> Self {
        match e {
            CatalogError::NotFound(m) => CatalogApiError::NotFound(m),
            CatalogError::AlreadyExists(m) => CatalogApiError::AlreadyExists(m),
            CatalogError::InUse => CatalogApiError::InUse,
            CatalogError::BadRequest(m) => CatalogApiError::BadRequest(m),
            CatalogError::Db(e) => CatalogApiError::Backend(e.to_string()),
        }
    }
}

#[async_trait::async_trait]
impl CatalogPort for CatalogStore {
    async fn get_warehouse(&self, id: Uuid) -> Result<Option<Warehouse>, CatalogApiError> {
        Ok(self.get_warehouse(id).await?.map(Into::into))
    }

    async fn get_warehouse_by_name(
        &self,
        name: &str,
    ) -> Result<Option<Warehouse>, CatalogApiError> {
        Ok(self.get_warehouse_by_name(name).await?.map(Into::into))
    }

    async fn list_roles(&self, warehouse_id: Uuid) -> Result<Vec<CatalogRole>, CatalogApiError> {
        Ok(self
            .list_roles(warehouse_id)
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    async fn create_role(
        &self,
        warehouse_id: Uuid,
        name: &str,
        consumer_type: &str,
        description: Option<&str>,
    ) -> Result<CatalogRole, CatalogApiError> {
        Ok(self
            .create_role(warehouse_id, name, consumer_type, description)
            .await?
            .into())
    }

    async fn add_grant(
        &self,
        catalog_role_id: Uuid,
        input: GrantInput,
    ) -> Result<CatalogGrant, CatalogApiError> {
        let g: concrete::GrantInput = input.into();
        Ok(self.add_grant(catalog_role_id, &g).await?.into())
    }

    async fn grants_for_role(
        &self,
        warehouse_name: &str,
        role_name: &str,
    ) -> Result<Vec<CatalogGrant>, CatalogApiError> {
        Ok(self
            .grants_for_role(warehouse_name, role_name)
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    async fn list_clients(
        &self,
        warehouse_id: Uuid,
    ) -> Result<Vec<CatalogClient>, CatalogApiError> {
        Ok(self
            .list_clients(warehouse_id)
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    async fn create_client(&self, client: NewClient) -> Result<CreatedClient, CatalogApiError> {
        let c: concrete::NewClient = client.into();
        Ok(self.create_client(&c).await?.into())
    }

    async fn revoke_client(&self, id: Uuid) -> Result<(), CatalogApiError> {
        self.revoke_client(id).await?;
        Ok(())
    }
}
