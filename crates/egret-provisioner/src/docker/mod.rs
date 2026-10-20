//! Docker engine provisioning: engines as plain containers instead of
//! Kubernetes Deployments. `plan` is the pure part (spec -> container plans).

pub mod api;
pub mod bollard_api;
pub mod plan;
pub mod provisioner;
pub mod transport;

pub use api::{ContainerInfo, ContainerState, DockerApi};
pub use bollard_api::BollardDocker;
pub use plan::{
    container_name, files_tar, image_allowed, parse_cpu, parse_memory, plan_trino, ContainerPlan,
    PlanFile, PlanOptions, PlanRole,
};
pub use provisioner::DockerEngineProvisioner;
pub use transport::HttpCoordinatorTransport;
