//! Engine provisioner trait.

use crate::spec::{EngineSpec, EngineStatus, RangerRollout};

/// Drives an engine through its lifecycle on Kubernetes. Implemented by
/// per-engine provisioners (e.g. `TrinoProvisioner`).
#[async_trait::async_trait]
pub trait EngineProvisioner: Send + Sync {
    async fn provision(&self, spec: &EngineSpec) -> anyhow::Result<()>;
    async fn status(&self, namespace: &str, name: &str) -> anyhow::Result<EngineStatus>;
    async fn update(&self, spec: &EngineSpec) -> anyhow::Result<()>;
    async fn delete(&self, namespace: &str, name: &str) -> anyhow::Result<()>;
    /// Whether every pod that runs the engine's Ranger plugin has rolled out
    /// with the credential `credential_id` ([`crate::rollout`]). Fails closed:
    /// anything not observed complete is `Pending`, a failed read an `Err`.
    async fn ranger_rollout(
        &self,
        namespace: &str,
        name: &str,
        credential_id: &str,
    ) -> anyhow::Result<RangerRollout>;
}
