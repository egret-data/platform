//! `PolicyPort` (contract crate) implemented over the concrete `AuthzStore`.
//! The in-process wiring; at a real split an HTTP client implements the same
//! trait. Converts `ServiceRow -> PolicyService` and `AuthzError -> PolicyError`
//! at the seam boundary so no sqlx type escapes the contract.
use crate::store::ServiceRow;
use crate::{AuthzError, AuthzStore};
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

impl From<AuthzError> for PolicyError {
    fn from(e: AuthzError) -> Self {
        match e {
            AuthzError::NotFound(m) => PolicyError::NotFound(m),
            AuthzError::AlreadyExists(m) => PolicyError::AlreadyExists(m),
            other => PolicyError::Backend(other.to_string()),
        }
    }
}

#[async_trait::async_trait]
impl PolicyPort for AuthzStore {
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
