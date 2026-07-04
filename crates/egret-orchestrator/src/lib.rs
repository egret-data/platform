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
    Affinity, Auth, CatalogSpec, ContainerPort, DeploymentStrategy, EngineSpec, EngineStatus,
    EnvFromSource, FaultTolerant, GracefulShutdown, HeapConfig, Ingress, Jwt, MiniContainer,
    NodeSelectorRequirement, ObjectMount, PasswordEntry, PodStatus, Probe, Probes, ResourceGroup,
    ResourceGroups, ResourceSelector, RoleConfig, RoleResources, SecurityContext, SessionRule,
    Toleration, TopologySpreadConstraint, TrinoConfig, VolumeMount, WorkerRoleConfig,
};
pub use trino::TrinoProvisioner;
