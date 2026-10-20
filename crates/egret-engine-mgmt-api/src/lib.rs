//! Canonical, dependency-free engine-provisioning spec DTOs (the E1 contract-crate
//! seed). Both `egret-controlplane` (HTTP + secret resolution) and
//! `egret-provisioner` (k8s rendering) depend on these — neither owns the model.
//! Kube-free by construction: this crate has no dependencies.

pub mod ranger_config;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;

/// A pod toleration allowing scheduling onto tainted nodes.
#[derive(Clone, Debug, Default)]
pub struct Toleration {
    pub key: Option<String>,
    pub operator: Option<String>,
    pub value: Option<String>,
    pub effect: Option<String>,
    pub toleration_seconds: Option<i64>,
}

/// A pod topology-spread constraint.
#[derive(Clone, Debug, Default)]
pub struct TopologySpreadConstraint {
    pub max_skew: u32,
    pub topology_key: String,
    pub when_unsatisfiable: String,
    pub match_labels: BTreeMap<String, String>,
}

/// A single node-selector match expression (used inside node affinity).
#[derive(Clone, Debug, Default)]
pub struct NodeSelectorRequirement {
    pub key: String,
    pub operator: String,
    pub values: Vec<String>,
}

/// Structured-common affinity with a raw-JSON override that wins verbatim.
#[derive(Clone, Debug, Default)]
pub struct Affinity {
    pub node_match: Vec<NodeSelectorRequirement>,
    pub pod_anti_affinity_topology_key: Option<String>,
    pub raw_override: Option<Value>,
}

/// An additional container port to expose.
#[derive(Clone, Debug, Default)]
pub struct ContainerPort {
    pub name: String,
    pub container_port: u16,
}

/// Pod- or container-level security context (flat fields + raw override).
#[derive(Clone, Debug, Default)]
pub struct SecurityContext {
    pub run_as_user: Option<i64>,
    pub run_as_group: Option<i64>,
    pub fs_group: Option<i64>,
    pub run_as_non_root: Option<bool>,
    pub raw_override: Option<Value>,
}

/// A volume mount inside a container.
#[derive(Clone, Debug, Default)]
pub struct VolumeMount {
    pub name: String,
    pub mount_path: String,
    pub sub_path: Option<String>,
}

/// A minimal init/sidecar container definition (structured-common + raw override).
#[derive(Clone, Debug, Default)]
pub struct MiniContainer {
    pub name: String,
    pub image: String,
    pub command: Vec<String>,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub volume_mounts: Vec<VolumeMount>,
    pub raw_override: Option<Value>,
}

/// A reference feeding container env from a ConfigMap or Secret.
#[derive(Clone, Debug, Default)]
pub struct EnvFromSource {
    pub config_map_ref: Option<String>,
    pub secret_ref: Option<String>,
}

/// A ConfigMap- or Secret-backed volume mount referenced by object name.
#[derive(Clone, Debug, Default)]
pub struct ObjectMount {
    pub name: String,
    pub mount_path: String,
    pub sub_path: Option<String>,
    pub default_mode: Option<i32>,
}

/// A Deployment update strategy.
#[derive(Clone, Debug, Default)]
pub struct DeploymentStrategy {
    pub kind: String,
    pub max_surge: Option<String>,
    pub max_unavailable: Option<String>,
}

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
    pub node_selector: BTreeMap<String, String>,
    pub tolerations: Vec<Toleration>,
    pub topology_spread: Vec<TopologySpreadConstraint>,
    pub affinity: Option<Affinity>,
    pub termination_grace_seconds: Option<u32>,
    pub additional_exposed_ports: Vec<ContainerPort>,
    // Per-role pod shape (mirrors the Trino chart's `coordinator.*` / `worker.*`
    // maps). Populated per role; for back-compat a legacy cluster-wide value is
    // applied to both roles at parse time.
    pub init_containers: Vec<MiniContainer>,
    pub sidecar_containers: Vec<MiniContainer>,
    pub config_mounts: Vec<ObjectMount>,
    pub secret_mounts: Vec<ObjectMount>,
    pub share_process_namespace: Option<bool>,
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
/// The control plane derives the hash; the provisioner never sees plaintext.
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

/// jwt-mode coordinator TLS. `keystore_pem` = cert+key PEM mounted as the Trino
/// https keystore; `port` = the https listener port.
#[derive(Clone, Debug, Default)]
pub struct Tls {
    pub keystore_pem: String,
    pub port: u16,
}

/// A Kubernetes Ingress exposing the coordinator's HTTP endpoint.
#[derive(Clone, Debug, Default)]
pub struct Ingress {
    pub enabled: bool,
    pub class_name: Option<String>,
    pub host: String,
    pub tls_secret: Option<String>,
    /// Typed `kubernetes.io/tls` secret backing the ingress TLS. When set, the
    /// provision step resolves it, projects a `{name}-ingress-tls` k8s Secret,
    /// and points `tls_secret` at that projected name.
    pub tls_cert_secret_ref: Option<Uuid>,
    /// Resolved cert/key PEMs, injected at provision (NOT persisted). The
    /// provisioner renders the `{name}-ingress-tls` Secret from these. Like the
    /// sibling `Tls` DTO these are plain runtime fields — this struct is not
    /// serde-serialized (its wire form is parsed/rendered by hand), so the PEMs
    /// never reach stored config.
    pub tls_cert_pem: Option<String>,
    pub tls_key_pem: Option<String>,
    pub annotations: BTreeMap<String, String>,
}

/// A KEDA cron trigger's fields (structured-common).
#[derive(Clone, Debug, Default)]
pub struct CronTrigger {
    pub start: String,
    pub end: String,
    pub desired_replicas: u32,
    pub timezone: String,
}

/// A KEDA prometheus trigger's fields (structured-common).
#[derive(Clone, Debug, Default)]
pub struct PrometheusTrigger {
    pub server_address: String,
    pub metric_name: String,
    pub query: String,
    pub threshold: String,
}

/// One KEDA ScaledObject trigger: a structured kind plus a raw-override escape hatch.
#[derive(Clone, Debug, Default)]
pub struct KedaTrigger {
    pub kind: String,
    pub cron: Option<CronTrigger>,
    pub prometheus: Option<PrometheusTrigger>,
    pub raw_override: Option<Value>,
}

/// KEDA ScaledObject config: replica bounds, timing, triggers, raw fallback.
#[derive(Clone, Debug, Default)]
pub struct KedaConfig {
    pub min_replica_count: u32,
    pub max_replica_count: u32,
    pub polling_interval: Option<u32>,
    pub cooldown_period: Option<u32>,
    pub triggers: Vec<KedaTrigger>,
    pub fallback_raw: Option<Value>,
}

/// Native HPA config: max replicas, CPU/memory targets, raw behavior override.
#[derive(Clone, Debug, Default)]
pub struct HpaConfig {
    pub max_replicas: u32,
    pub target_cpu_percent: Option<u32>,
    pub target_memory_percent: Option<u32>,
    pub behavior_raw: Option<Value>,
}

/// Worker autoscaling: mode-exclusive native HPA or KEDA ScaledObject.
#[derive(Clone, Debug, Default)]
pub struct Autoscaling {
    pub mode: String,
    pub hpa: Option<HpaConfig>,
    pub keda: Option<KedaConfig>,
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
    pub tls: Option<Tls>,
    pub ingress: Option<Ingress>,
    /// Full container image (registry/repository:tag). When None, the renderer
    /// falls back to `trinodb/trino:{image_version}`.
    pub image: Option<String>,
    pub image_pull_secrets: Vec<String>,
    /// Typed `kubernetes.io/dockerconfigjson` secret backing an image-pull
    /// secret. When set, the provision step resolves it, projects a
    /// `{name}-pull` k8s Secret, and appends that name to `image_pull_secrets`.
    pub image_pull_secret_ref: Option<Uuid>,
    /// Resolved `.dockerconfigjson` payload, injected at provision (NOT
    /// persisted). The provisioner renders the `{name}-pull` Secret from this.
    /// Like the sibling runtime fields, this struct is not serde-serialized
    /// (its wire form is parsed/rendered by hand), so the payload never reaches
    /// stored config.
    pub image_pull_secret_json: Option<String>,
    pub image_pull_policy: Option<String>,
    pub service_account: Option<String>,
    pub pod_security_context: Option<SecurityContext>,
    pub container_security_context: Option<SecurityContext>,
    // Cluster-wide extra env sources (the chart's top-level `envFrom`). Plain env
    // vars live in `env` above. Per-role init/sidecar containers and config/secret
    // mounts now live on `RoleConfig` (coordinator/worker), matching the chart.
    pub env_from: Vec<EnvFromSource>,
    pub deployment_strategy: Option<DeploymentStrategy>,
    pub deployment_annotations: BTreeMap<String, String>,
    pub revision_history_limit: Option<u32>,
    pub progress_deadline_seconds: Option<u32>,
    pub autoscaling: Option<Autoscaling>,
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

/// Ranger tag/policy enforcement wiring (Access P3.2). `Some` when the engine is
/// bound to a policy profile; the provisioner then templates
/// `access-control.properties` + `ranger-trino-security.xml` so the bundled
/// `apache-ranger` plugin polls egret's Ranger-Admin `/service` download API.
#[derive(Clone, Debug)]
pub struct RangerBinding {
    /// `ranger.service.name` — the bound trino profile's name.
    pub service_name: String,
    /// Base URL the plugin polls; egret appends `/service/...`. Reuse `engine_egret_url`.
    pub egret_url: String,
    /// Policy/tag poll interval in ms (default 30_000).
    pub poll_interval_ms: u32,
}

/// Which query engine an `EngineSpec` describes. Selects the provisioner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EngineKind {
    #[default]
    Trino,
    StarRocks,
}
impl EngineKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EngineKind::Trino => "trino",
            EngineKind::StarRocks => "starrocks",
        }
    }
    pub fn parse(s: &str) -> Option<EngineKind> {
        match s {
            "trino" => Some(EngineKind::Trino),
            "starrocks" => Some(EngineKind::StarRocks),
            _ => None,
        }
    }
}

/// CN (compute-node) horizontal autoscaling target. Rendered into the
/// StarRocksCluster CR's `starRocksCnSpec.autoScalingPolicy`. `max_replicas` is
/// the only required knob; the rest fall back to operator defaults when None.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CnAutoscaling {
    pub min_replicas: Option<u32>,
    pub max_replicas: u32,
    pub target_cpu_percent: Option<u32>,
}

/// Pod- and container-level scheduling / customization surface applied to the
/// FE + CN specs of a provisioned StarRocks. Every field is optional (absent ⇒
/// operator default); the raw-JSON fields are passed through verbatim to give
/// full escape-hatch control without modelling every k8s sub-object here.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StarRocksPod {
    #[serde(default)]
    pub service_account: Option<String>,
    #[serde(default)]
    pub node_selector: BTreeMap<String, String>,
    #[serde(default)]
    pub annotations: BTreeMap<String, String>,
    #[serde(default)]
    pub pod_labels: BTreeMap<String, String>,
    #[serde(default)]
    pub env: Vec<Value>,
    #[serde(default)]
    pub tolerations: Option<Value>,
    #[serde(default)]
    pub affinity: Option<Value>,
    /// Reserved — NOT rendered: the kube-starrocks operator has no role-level
    /// `securityContext` field (it lives per-container inside
    /// `initContainers`/`sidecars`). Kept for forward-compat.
    #[serde(default)]
    pub security_context: Option<Value>,
    #[serde(default)]
    pub init_containers: Option<Value>,
    #[serde(default)]
    pub sidecars: Option<Value>,
    #[serde(default)]
    pub image_pull_secrets: Vec<String>,
}

/// `fe.conf` keys egret manages itself (shared-data run mode, object-store
/// coordinates, TLS keystore, and the wire ports). A user-supplied `feConfig`
/// override for any of these is rejected up front so it can't clobber the
/// provisioner's own wiring. Shared with the provisioner (which renders the
/// merged `fe.conf`) via this contract crate so both sides agree on the set.
pub const SR_PROTECTED_FE_KEYS: &[&str] = &[
    "run_mode",
    "cloud_native_meta_port",
    "cloud_native_storage_type",
    "aws_s3_endpoint",
    "aws_s3_path",
    "aws_s3_access_key",
    "aws_s3_secret_key",
    "aws_s3_use_aws_sdk_default_behavior",
    "aws_s3_enable_path_style_access",
    "enable_load_volume_from_conf",
    "access_control",
    "ssl_keystore_location",
    "ssl_keystore_password",
    "ssl_key_password",
    "ssl_force_secure_transport",
    "http_port",
    "rpc_port",
    "query_port",
    "edit_log_port",
];

/// StarRocks shared-data engine config (FE + CN). Storage params drive the
/// FE builtin storage volume (object store); FE/CN resources default when None.
///
/// `Debug` is hand-written to redact the object-store credentials so `{:?}`
/// on this (or an enclosing `EngineSpec`) never leaks the access/secret keys.
#[derive(Clone)]
pub struct StarRocksConfig {
    pub fe_image: String, // starrocks/fe-ubuntu:<ver>
    pub cn_image: String, // starrocks/cn-ubuntu:<ver>
    pub fe_replicas: u32,
    pub cn_replicas: u32,
    pub storage_endpoint: String, // aws_s3_endpoint
    pub storage_path: String,     // aws_s3_path = bucket/prefix
    pub storage_access_key: String,
    pub storage_secret_key: String,
    /// FE (metadata) role sizing; defaults when None.
    pub fe_resources: Option<RoleResources>,
    /// CN (compute) role sizing; defaults when None.
    pub cn_resources: Option<RoleResources>,
    /// Delegation/security wiring (JWT integration + FE TLS); `None` = neither
    /// templated (slice-2 behavior, open engine).
    pub delegation: Option<StarRocksDelegation>,
    /// FE (`fe.conf`) override keys merged over egret's managed base; the
    /// managed keys ([`SR_PROTECTED_FE_KEYS`]) are guarded away at the API.
    pub fe_config: BTreeMap<String, String>,
    /// CN (`cn.conf`) override keys merged over egret's managed base.
    pub cn_config: BTreeMap<String, String>,
    /// CN horizontal autoscaling target; `None` = fixed `cn_replicas`.
    pub cn_autoscaling: Option<CnAutoscaling>,
    /// Pod- and container-level scheduling / customization; `None` = defaults.
    pub pod: Option<StarRocksPod>,
    /// FE `root` account password set in-cluster via the operator-style
    /// initpwd Secret/ConfigMap/Job (see egret-provisioner's
    /// `starrocks_initpwd_*`). `None`/empty = legacy passwordless root (no
    /// initpwd resources templated). Minted by the create path (T3).
    pub root_password: Option<String>,
    /// StorageClass backing the FE metadata PVC (`/opt/starrocks/fe/meta`).
    /// `None`/blank ⇒ the field is omitted from the CR, so the cluster's
    /// DEFAULT StorageClass binds the claim (on kind: `standard`,
    /// `rancher.io/local-path`).
    pub fe_meta_storage_class: Option<String>,
    /// Size of the FE metadata PVC (a k8s quantity, e.g. `"20Gi"`). `None`
    /// ⇒ [`DEFAULT_FE_META_STORAGE_SIZE`]. The CRD validates the string
    /// against `(^0|([0-9]*l[.])?[0-9]+((M|G|T|E|P)i))$`, so only binary
    /// suffixes are accepted.
    pub fe_meta_storage_size: Option<String>,
    /// Whether egret renders the FE metadata volume at all.
    ///
    /// `false` ⇒ render NOTHING at the meta path, so the kube-starrocks
    /// operator injects its own `fe-meta {"emptyDir": {}}` default — the exact
    /// shape egret emitted before the PVC existed. `true` ⇒ render the PVC.
    ///
    /// This is a CREATION-TIME marker, not a knob: `volumeClaimTemplates` is
    /// IMMUTABLE on a live StatefulSet, measured on kind 2026-09-26 (see
    /// `docs/en/plans/2026-09-26-fe-meta-live-result.md` §7). Adding the claim
    /// to an engine that already has a StatefulSet does not migrate it — every
    /// operator reconcile is rejected by the API server with `updates to
    /// statefulset spec for fields other than 'replicas', 'ordinals',
    /// 'template', ... are forbidden`, the FE keeps its `emptyDir`, AND the
    /// `StarRocksCluster` stops converging entirely, so no later change of any
    /// kind (a Ranger rebind included) applies either. Existing engines
    /// therefore keep `false` and are left exactly as they are; migrating one
    /// is a deliberate operator action (`kubectl delete sts <engine>-fe
    /// --cascade=orphan`, which destroys that FE's metadata), never a side
    /// effect of upgrading egret.
    pub fe_meta_persistent: bool,
}

/// Default size of the FE metadata PVC when
/// [`StarRocksConfig::fe_meta_storage_size`] is `None`. FE metadata (the BDBJE
/// journal + image) is small — single-digit GiB even for a large catalog — so
/// this is deliberately modest; `local-path` on kind ignores the request
/// anyway (it hands out a host directory).
pub const DEFAULT_FE_META_STORAGE_SIZE: &str = "20Gi";

/// Delegation/security wiring for a provisioned StarRocks. `None` on
/// [`StarRocksConfig::delegation`] = no JWT/TLS templated.
///
/// `Debug` is hand-written (see below) to redact `tls_keystore_pem`, which
/// carries the FE's private key alongside the cert — `tls_cert_pem` is the
/// public cert alone and prints fine.
#[derive(Clone)]
pub struct StarRocksDelegation {
    /// egret machine-IdP JWKS URL (the STS issuer engines trust), reachable
    /// from the FE pod.
    pub jwks_url: String,
    pub issuer: String,
    pub audience: String,
    /// PEM of the FE TLS cert egret generated (also pinned on the connection).
    pub tls_cert_pem: String,
    /// Cert+key PEM bundle (`ensure_tls_cert`'s `keystore_pem`) — the source
    /// material the provisioner's init-container converts into the Java
    /// keystore `ssl_keystore_location` points at. Contains the private key.
    pub tls_keystore_pem: String,
    /// Enable FE SSL on :9030.
    pub tls_enabled: bool,
}

impl std::fmt::Debug for StarRocksDelegation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StarRocksDelegation")
            .field("jwks_url", &self.jwks_url)
            .field("issuer", &self.issuer)
            .field("audience", &self.audience)
            .field("tls_cert_pem", &self.tls_cert_pem)
            .field("tls_keystore_pem", &"<redacted>")
            .field("tls_enabled", &self.tls_enabled)
            .finish()
    }
}

impl std::fmt::Debug for StarRocksConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StarRocksConfig")
            .field("fe_image", &self.fe_image)
            .field("cn_image", &self.cn_image)
            .field("fe_replicas", &self.fe_replicas)
            .field("cn_replicas", &self.cn_replicas)
            .field("storage_endpoint", &self.storage_endpoint)
            .field("storage_path", &self.storage_path)
            .field("storage_access_key", &"<redacted>")
            .field("storage_secret_key", &"<redacted>")
            .field("fe_resources", &self.fe_resources)
            .field("cn_resources", &self.cn_resources)
            .field("delegation", &self.delegation)
            .field("fe_config", &self.fe_config)
            .field("cn_config", &self.cn_config)
            .field("cn_autoscaling", &self.cn_autoscaling)
            .field("pod", &self.pod)
            .field(
                "root_password",
                if self.root_password.is_some() {
                    &"<redacted>"
                } else {
                    &"None"
                },
            )
            .field("fe_meta_storage_class", &self.fe_meta_storage_class)
            .field("fe_meta_storage_size", &self.fe_meta_storage_size)
            .field("fe_meta_persistent", &self.fe_meta_persistent)
            .finish()
    }
}

/// Desired state of a query engine (`kind` selects Trino vs StarRocks).
#[derive(Clone, Debug)]
pub struct EngineSpec {
    pub name: String, // DNS-1123 k8s name
    pub namespace: String,
    pub image_version: String, // official trinodb/trino tag, e.g. "470"
    pub config: TrinoConfig,
    pub catalogs: Vec<CatalogSpec>,
    /// Ranger enforcement wiring; `None` = no access control templated (open engine).
    pub ranger: Option<RangerBinding>,
    /// Engine type — selects the provisioner. Defaults to Trino.
    pub kind: EngineKind,
    /// Present when `kind == StarRocks`; the Trino `config`/`catalogs` are unused then.
    pub starrocks: Option<StarRocksConfig>,
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

use async_trait::async_trait;

/// Provisions/updates/deletes a query engine on Kubernetes. Kube-free by
/// construction (this crate has no kube dep). Implemented in the composition
/// root (egret-bin) over `egret-provisioner::TrinoProvisioner`.
#[async_trait]
pub trait EngineProvisioningPort: Send + Sync {
    async fn provision(&self, req: &EngineSpec) -> Result<(), String>;
    async fn status(&self, kind: &str, namespace: &str, name: &str)
        -> Result<EngineStatus, String>;
    async fn update(&self, req: &EngineSpec) -> Result<(), String>;
    async fn delete(&self, kind: &str, namespace: &str, name: &str) -> Result<(), String>;
    async fn restart(&self, kind: &str, namespace: &str, name: &str) -> Result<(), String>;
}

/// Sends one HTTP request to an engine's coordinator without Kubernetes (the
/// Docker engine provisioner). The coordinator is addressed as
/// `service.namespace` on port 8080, exactly as in-cluster.
#[async_trait]
pub trait CoordinatorTransport: Send + Sync {
    async fn request(
        &self,
        namespace: &str,
        service: &str,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: Option<Vec<u8>>,
    ) -> Result<(u16, Vec<u8>), String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_kind_parses_and_defaults_trino() {
        assert_eq!(EngineKind::default(), EngineKind::Trino);
        assert_eq!(EngineKind::parse("starrocks"), Some(EngineKind::StarRocks));
        assert_eq!(EngineKind::parse("trino"), Some(EngineKind::Trino));
        assert_eq!(EngineKind::StarRocks.as_str(), "starrocks");
    }

    #[test]
    fn engine_spec_carries_starrocks_config() {
        let s = StarRocksConfig {
            fe_image: "starrocks/fe-ubuntu:3.3-latest".into(),
            cn_image: "starrocks/cn-ubuntu:3.3-latest".into(),
            fe_replicas: 1,
            cn_replicas: 1,
            storage_endpoint: "http://host.docker.internal:9000".into(),
            storage_path: "warehouse/starrocks".into(),
            storage_access_key: "egret".into(),
            storage_secret_key: "egretsecret".into(),
            fe_resources: None,
            cn_resources: None,
            delegation: None,
            fe_config: Default::default(),
            cn_config: Default::default(),
            cn_autoscaling: None,
            pod: None,
            root_password: None,
            fe_meta_storage_class: None,
            fe_meta_storage_size: None,
            fe_meta_persistent: true,
        };
        assert_eq!(s.fe_replicas, 1);
    }
}
