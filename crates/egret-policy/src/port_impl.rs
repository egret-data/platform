//! `PolicyPort` (contract crate) implemented over the concrete `PolicyStore`.
//! The in-process wiring; at a real split an HTTP client implements the same
//! trait. Converts `ServiceRow -> PolicyService` and `PolicyStoreError -> PolicyError`
//! at the seam boundary so no sqlx type escapes the contract.
use crate::store::ServiceRow;
use crate::{PolicyStore, PolicyStoreError};
use egret_policy_api::{PolicyError, PolicyPort, PolicyService};
use serde_json::Value;

impl From<ServiceRow> for PolicyService {
    fn from(r: ServiceRow) -> Self {
        PolicyService {
            id: r.id,
            name: r.name,
            r#type: r.r#type,
        }
    }
}

impl From<PolicyStoreError> for PolicyError {
    fn from(e: PolicyStoreError) -> Self {
        match e {
            PolicyStoreError::NotFound(m) => PolicyError::NotFound(m),
            PolicyStoreError::AlreadyExists(m) => PolicyError::AlreadyExists(m),
            other => PolicyError::Backend(other.to_string()),
        }
    }
}

#[async_trait::async_trait]
impl PolicyPort for PolicyStore {
    async fn ensure_trino_def(&self) -> Result<(), PolicyError> {
        crate::service_def::seed_trino(self)
            .await
            .map_err(Into::into)
    }

    async fn list_services(&self) -> Result<Vec<PolicyService>, PolicyError> {
        Ok(self
            .list_services()
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    async fn create_service(
        &self,
        name: &str,
        type_: &str,
        config: &Value,
    ) -> Result<PolicyService, PolicyError> {
        Ok(self.create_service(name, type_, config).await?.into())
    }
}
