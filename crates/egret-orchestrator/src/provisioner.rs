//! Engine provisioner trait. Filled in a later task.

/// Drives an engine through its lifecycle on Kubernetes. Implemented by
/// per-engine provisioners (e.g. `TrinoProvisioner`).
pub trait EngineProvisioner: Send + Sync {}
