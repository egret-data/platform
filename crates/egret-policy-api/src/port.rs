//! `PolicyPort` — the cross-seam interface into the Policy/Ranger store.
//!
//! Only the calls that reach the policy store from *outside* the seam belong
//! here (today: engine-mgmt's engine-create auto-bind). In-seam callers (the
//! Ranger router, the admin policy editor) use the concrete store directly.
//! At a real service split, an HTTP client implements this same trait over the
//! same DTOs — no call-site churn.
use crate::dto::PolicyService;
use serde_json::Value;

/// Transport-neutral policy-seam error (no sqlx leak across the boundary).
#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    #[error("policy resource not found: {0}")]
    NotFound(String),
    #[error("policy resource already exists: {0}")]
    AlreadyExists(String),
    #[error("policy store error: {0}")]
    Backend(String),
}

#[async_trait::async_trait]
pub trait PolicyPort: Send + Sync {
    /// Ensure the built-in `trino` service-def is seeded. Idempotent.
    async fn ensure_trino_def(&self) -> Result<(), PolicyError>;

    /// List all policy profiles (services).
    async fn list_services(&self) -> Result<Vec<PolicyService>, PolicyError>;

    /// Create a new policy profile. Errors `AlreadyExists` on a name clash.
    async fn create_service(
        &self,
        name: &str,
        type_: &str,
        config: &Value,
    ) -> Result<PolicyService, PolicyError>;
}
