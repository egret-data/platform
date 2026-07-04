//! Engine-agnostic provisioning spec + status (no k8s types leak out).

use std::collections::BTreeMap;

/// JVM heap sizing for a role: either a fixed size (`-Xmx`) or a RAM percentage.
#[derive(Clone, Debug, Default)]
pub struct HeapConfig {
    pub max_size: Option<String>,    // "-Xmx" when set
    pub min_size: Option<String>,    // "-Xms"
    pub max_percent: Option<u8>,     // -XX:MaxRAMPercentage (used only if max_size is None)
    pub initial_percent: Option<u8>, // -XX:InitialRAMPercentage
}

/// CPU/memory requests + limits for a role.
#[derive(Clone, Debug, Default)]
pub struct RoleResources {
    pub cpu_request: Option<String>,
    pub cpu_limit: Option<String>,
    pub mem_request: Option<String>,
    pub mem_limit: Option<String>,
}

/// A single Kubernetes probe's tunable fields.
#[derive(Clone, Debug, Default)]
pub struct Probe {
    pub initial_delay_seconds: Option<u32>,
    pub period_seconds: Option<u32>,
    pub timeout_seconds: Option<u32>,
    pub failure_threshold: Option<u32>,
    pub success_threshold: Option<u32>,
}

/// The three probe kinds for a role.
#[derive(Clone, Debug, Default)]
pub struct Probes {
    pub liveness: Option<Probe>,
    pub readiness: Option<Probe>,
    pub startup: Option<Probe>,
}

/// Per-role config shared by coordinator and worker.
#[derive(Clone, Debug, Default)]
pub struct RoleConfig {
    pub replicas: u32,
    pub resources: RoleResources,
    pub heap: HeapConfig,
    pub query_max_memory_per_node: Option<String>,
    pub additional_jvm_config: Vec<String>,
    pub probes: Probes,
}

/// Worker graceful-shutdown drain settings.
#[derive(Clone, Debug, Default)]
pub struct GracefulShutdown {
    pub enabled: bool,
    pub grace_period_seconds: u32,
}

/// Worker role: base role config plus optional graceful shutdown.
#[derive(Clone, Debug, Default)]
pub struct WorkerRoleConfig {
    pub base: RoleConfig,
    pub graceful_shutdown: Option<GracefulShutdown>,
}

/// Fuller Trino config surface, split per-role (coordinator/worker independent).
#[derive(Clone, Debug, Default)]
pub struct TrinoConfig {
    pub query_max_memory: Option<String>, // query.max-memory (cluster-wide)
    pub log_level: Option<String>,        // io.trino log level
    pub config_properties: BTreeMap<String, String>, // raw config.properties
    pub env: BTreeMap<String, String>,    // extra env on both roles
    pub coordinator: RoleConfig,
    pub worker: WorkerRoleConfig,
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
