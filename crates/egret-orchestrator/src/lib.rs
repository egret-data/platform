//! egret-orchestrator: provisions query engines onto Kubernetes via operators.
//! Phase 1: Trino (Stackable operator). Engines are described by [`EngineSpec`]
//! and driven through the [`EngineProvisioner`] trait; k8s access goes through the
//! [`KubeClient`] trait so tests can inject a fake.

pub mod kube_client;
pub mod provisioner;
pub mod spec;
pub mod trino;

pub use kube_client::{KubeClient, LiveKube};
pub use provisioner::EngineProvisioner;
pub use spec::{
    CatalogSpec, EngineSpec, EngineStatus, GracefulShutdown, HeapConfig, PodStatus, Probe, Probes,
    RoleConfig, RoleResources, TrinoConfig, WorkerRoleConfig,
};
pub use trino::TrinoProvisioner;
