//! egret-provisioner: provisions query engines onto Kubernetes via operators.
//! Phase 1: Trino (Stackable operator). Engines are described by [`EngineSpec`]
//! and driven through the [`EngineProvisioner`] trait; k8s access goes through the
//! [`KubeClient`] trait so tests can inject a fake.

pub mod docker;
pub mod ingress;
pub mod kube_client;
pub mod provisioner;
pub mod spec;
pub mod starrocks;
pub mod trino;

pub use kube_client::{KubeClient, LiveKube};
pub use provisioner::EngineProvisioner;
// The renderer lives in `egret-engine-mgmt-api` (the lowest crate both the
// provisioner and the policy admin route can reach); re-exported so there is one
// spelling and the provisioner's callers keep their paths.
pub use egret_engine_mgmt_api::ranger_config;
pub use ranger_config::{ranger_plugin_files, RangerPluginFiles};
pub use spec::{
    Affinity, Auth, Autoscaling, CatalogSpec, ContainerPort, CronTrigger, DeploymentStrategy,
    EngineSpec, EngineStatus, EnvFromSource, FaultTolerant, GracefulShutdown, HeapConfig,
    HpaConfig, Ingress, Jwt, KedaConfig, KedaTrigger, MiniContainer, NodeSelectorRequirement,
    ObjectMount, PasswordEntry, PodStatus, Probe, Probes, PrometheusTrigger, ResourceGroup,
    ResourceGroups, ResourceSelector, RoleConfig, RoleResources, SecurityContext, SessionRule,
    Toleration, TopologySpreadConstraint, TrinoConfig, VolumeMount, WorkerRoleConfig,
};
pub use starrocks::StarRocksProvisioner;
pub use trino::TrinoProvisioner;
