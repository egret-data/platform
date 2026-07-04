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
    pub heap_headroom_per_node: Option<String>,
}

/// One resource group in the file-based resource-group manager tree.
#[derive(Clone, Debug, Default)]
pub struct ResourceGroup {
    pub name: String,
    pub soft_memory_limit: String, // "80%" or "10GB"
    pub hard_concurrency_limit: u32,
    pub max_queued: u32,
    pub scheduling_policy: Option<String>,
    pub sub_groups: Vec<ResourceGroup>, // recursive
}

/// A selector mapping incoming queries onto a resource group.
#[derive(Clone, Debug, Default)]
pub struct ResourceSelector {
    pub user: Option<String>,
    pub source: Option<String>,
    pub query_type: Option<String>,
    pub group: String, // dot-path, e.g. "global.etl"
}

/// The full file-based resource-group config: root groups + selectors.
#[derive(Clone, Debug, Default)]
pub struct ResourceGroups {
    pub root_groups: Vec<ResourceGroup>,
    pub selectors: Vec<ResourceSelector>,
}

/// One rule in the file-based session-property manager.
#[derive(Clone, Debug, Default)]
pub struct SessionRule {
    pub match_user: Option<String>,
    pub match_source: Option<String>,
    pub match_client_tags: Vec<String>,
    pub properties: BTreeMap<String, String>,
}

/// Fault-tolerant execution config: retry policy + S3 exchange manager.
#[derive(Clone, Debug, Default)]
pub struct FaultTolerant {
    pub retry_policy: String, // "TASK" | "QUERY"
    pub s3_base_dir: String,  // "s3://bucket/prefix"
    pub s3_endpoint: String,
    pub s3_region: String,
    pub s3_path_style: bool,
    // creds are provided via TrinoConfig.env (EXCHANGE_S3_ACCESS_KEY / _SECRET_KEY)
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

/// One PASSWORD-authenticator user: username plus a bcrypt hash of the password.
/// The control plane derives the hash; the orchestrator never sees plaintext.
#[derive(Clone, Debug, Default)]
pub struct PasswordEntry {
    pub username: String,
    pub bcrypt_hash: String,
}

/// JWT authenticator config: JWKS URI plus an optional required audience.
#[derive(Clone, Debug, Default)]
pub struct Jwt {
    pub jwks_uri: String,
    pub required_audience: Option<String>,
}

/// Trino authentication: file PASSWORD users and/or a JWT authenticator.
#[derive(Clone, Debug, Default)]
pub struct Auth {
    pub password_users: Vec<PasswordEntry>,
    pub jwt: Option<Jwt>,
}

/// A Kubernetes Ingress exposing the coordinator's HTTP endpoint.
#[derive(Clone, Debug, Default)]
pub struct Ingress {
    pub enabled: bool,
    pub class_name: Option<String>,
    pub host: String,
    pub tls_secret: Option<String>,
    pub annotations: BTreeMap<String, String>,
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
    pub additional_node_properties: BTreeMap<String, String>,
    pub additional_log_properties: BTreeMap<String, String>,
    pub event_listener: BTreeMap<String, String>,
    pub resource_groups: Option<ResourceGroups>,
    pub session_properties: Vec<SessionRule>,
    pub fault_tolerant: Option<FaultTolerant>,
    pub auth: Option<Auth>,
    pub ingress: Option<Ingress>,
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
