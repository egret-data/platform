//! `CatalogPort` — the cross-seam interface into the embedded-catalog store.
//!
//! Only the calls that reach the catalog store from *outside* the seam belong
//! here (today: engine-mgmt's engine-create auto-wiring of a Catalog Client, plus
//! the catalog-guard/Authorizer reads consumed at the composition root). In-seam
//! callers (the control-plane warehouse admin) use the concrete store directly.
//! At a real service split, an HTTP client implements this same trait over the
//! same DTOs — no call-site churn.
use crate::dto::{
    CatalogClient, CatalogGrant, CatalogRole, CreatedClient, GrantInput, NewClient, Warehouse,
};
use uuid::Uuid;

/// Transport-neutral catalog-seam error (no sqlx leak across the boundary).
///
/// Naming note: unlike the policy seam (`PolicyError` in the contract crate,
/// `PolicyStoreError` on the concrete store), here the concrete store keeps its
/// established, widely-used public name `egret_catalog_engine::CatalogError` and the
/// boundary type is `CatalogApiError` — renaming the concrete type would be large
/// churn for no gain, and `...ApiError` reads clearly as "the contract boundary".
#[derive(Debug, thiserror::Error)]
pub enum CatalogApiError {
    #[error("catalog resource not found: {0}")]
    NotFound(String),
    #[error("catalog resource already exists: {0}")]
    AlreadyExists(String),
    #[error("catalog resource is in use")]
    InUse,
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("catalog store error: {0}")]
    Backend(String),
}

#[async_trait::async_trait]
pub trait CatalogPort: Send + Sync {
    /// Look up a warehouse by id.
    async fn get_warehouse(&self, id: Uuid) -> Result<Option<Warehouse>, CatalogApiError>;

    /// Look up a warehouse by name (the key engines connect by).
    async fn get_warehouse_by_name(&self, name: &str)
        -> Result<Option<Warehouse>, CatalogApiError>;

    /// All catalog roles of a warehouse.
    async fn list_roles(&self, warehouse_id: Uuid) -> Result<Vec<CatalogRole>, CatalogApiError>;

    /// Create a catalog role. Errors `AlreadyExists` on a name clash.
    async fn create_role(
        &self,
        warehouse_id: Uuid,
        name: &str,
        consumer_type: &str,
        description: Option<&str>,
    ) -> Result<CatalogRole, CatalogApiError>;

    /// Add a privilege grant to a role. Errors `AlreadyExists` on a duplicate.
    async fn add_grant(
        &self,
        catalog_role_id: Uuid,
        input: GrantInput,
    ) -> Result<CatalogGrant, CatalogApiError>;

    /// All grants for a role identified by (warehouse name, role name) — the
    /// lookup the Authorizer uses from a JWT's `warehouse`+`roles` claims.
    async fn grants_for_role(
        &self,
        warehouse_name: &str,
        role_name: &str,
    ) -> Result<Vec<CatalogGrant>, CatalogApiError>;

    /// All Catalog Clients of a warehouse.
    async fn list_clients(&self, warehouse_id: Uuid)
        -> Result<Vec<CatalogClient>, CatalogApiError>;

    /// Mint a Catalog Client; returns the plaintext secret ONCE.
    async fn create_client(&self, client: NewClient) -> Result<CreatedClient, CatalogApiError>;

    /// Retire a Catalog Client by id.
    async fn revoke_client(&self, id: Uuid) -> Result<(), CatalogApiError>;
}
