//! Engine-agnostic provisioning spec + status (no k8s types leak out).

use std::collections::BTreeMap;

/// CPU/memory request for a Stackable role group.
#[derive(Clone, Debug)]
pub struct Resources {
    pub cpu_min: String, // e.g. "500m"
    pub cpu_max: String, // e.g. "2"
    pub memory: String,  // e.g. "4Gi"
}

/// Fuller Trino config surface (curated + raw). Curated keys win over raw on conflict.
#[derive(Clone, Debug, Default)]
pub struct TrinoConfig {
    pub jvm_heap: Option<String>,                  // e.g. "2G" -> -Xmx/-Xms
    pub query_max_memory: Option<String>,          // query.max-memory
    pub query_max_memory_per_node: Option<String>, // query.max-memory-per-node
    pub config_properties: BTreeMap<String, String>, // raw config.properties
    pub env: BTreeMap<String, String>,             // extra env on both roles
}

/// One catalog rendered as `<name>.properties`. `properties` may contain
/// `${ENV:VAR}` references; `env` supplies those vars (resolved secrets) and is
/// merged into the engine's env Secret.
#[derive(Clone, Debug)]
pub struct CatalogSpec {
    pub name: String,
    pub properties: BTreeMap<String, String>,
    pub env: BTreeMap<String, String>,
}

/// Desired state of a Trino engine.
#[derive(Clone, Debug)]
pub struct EngineSpec {
    pub name: String, // DNS-1123 k8s name
    pub namespace: String,
    pub image_version: String, // official trinodb/trino tag, e.g. "470"
    pub coordinator_replicas: u32,
    pub worker_replicas: u32,
    pub resources: Resources,
    pub config: TrinoConfig,
    pub catalogs: Vec<CatalogSpec>,
}

#[derive(Clone, Debug, Default)]
pub struct PodStatus {
    pub name: String,
    pub ready: bool,
    pub phase: String,
}

#[derive(Clone, Debug, Default)]
pub struct EngineStatus {
    /// High-level phase: "provisioning" | "running" | "failed" | "unknown".
    pub phase: String,
    pub message: Option<String>,
    pub pods: Vec<PodStatus>,
}
