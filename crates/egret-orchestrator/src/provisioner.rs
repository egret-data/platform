//! Engine provisioner trait.

use crate::spec::{EngineSpec, EngineStatus};

/// Drives an engine through its lifecycle on Kubernetes. Implemented by
/// per-engine provisioners (e.g. `TrinoProvisioner`).
#[async_trait::async_trait]
pub trait EngineProvisioner: Send + Sync {
    async fn provision(&self, spec: &EngineSpec) -> anyhow::Result<()>;
    async fn status(&self, namespace: &str, name: &str) -> anyhow::Result<EngineStatus>;
    async fn update(&self, spec: &EngineSpec) -> anyhow::Result<()>;
    async fn delete(&self, namespace: &str, name: &str) -> anyhow::Result<()>;
}
