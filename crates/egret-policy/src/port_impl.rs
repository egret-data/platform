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

    async fn ensure_tag_def(&self) -> Result<(), PolicyError> {
        crate::service_def::seed_tag(self).await.map_err(Into::into)
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

    async fn usable_tag_service(&self, service_id: i64) -> Result<Option<String>, PolicyError> {
        Ok(self.usable_tag_service(service_id).await?)
    }

    async fn get_service_opt(&self, name: &str) -> Result<Option<PolicyService>, PolicyError> {
        Ok(self.get_service_opt(name).await?.map(Into::into))
    }

    async fn delete_service(&self, id: i64) -> Result<(), PolicyError> {
        Ok(self.delete_service(id).await?)
    }

    async fn policy_exists(&self, service_id: i64, name: &str) -> Result<bool, PolicyError> {
        Ok(self.policy_exists(service_id, name).await?)
    }

    async fn create_policy(
        &self,
        service_id: i64,
        name: &str,
        policy: &Value,
    ) -> Result<(), PolicyError> {
        self.create_policy(service_id, name, policy).await?;
        Ok(())
    }

    async fn create_infra_policy(
        &self,
        service_id: i64,
        name: &str,
        policy: &Value,
    ) -> Result<(), PolicyError> {
        self.create_infra_policy(service_id, name, policy).await?;
        Ok(())
    }

    async fn seed_admin_all_access(
        &self,
        service_id: i64,
        service_name: &str,
        service_type: &str,
        users: &[String],
    ) -> Result<usize, PolicyError> {
        Ok(self
            .seed_admin_all_access(service_id, service_name, service_type, users)
            .await?)
    }
}
