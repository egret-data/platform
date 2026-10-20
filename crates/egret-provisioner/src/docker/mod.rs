//! Docker engine provisioning: engines as plain containers instead of
//! Kubernetes Deployments. `plan` is the pure part (spec -> container plans).

pub mod plan;

pub use plan::{
    container_name, files_tar, image_allowed, parse_cpu, parse_memory, plan_trino, ContainerPlan,
    PlanFile, PlanOptions, PlanRole,
};
