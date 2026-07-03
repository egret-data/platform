//! Engine-agnostic provisioning spec + status (no k8s types leak out).

/// CPU/memory request for a Stackable role group.
#[derive(Clone, Debug)]
pub struct Resources {
    pub cpu_min: String, // e.g. "500m"
    pub cpu_max: String, // e.g. "2"
    pub memory: String,  // e.g. "4Gi"
}

/// One extra Trino catalog property (raw value only; the wired egret catalog is
/// built separately with a Secret-backed credential).
#[derive(Clone, Debug)]
pub struct CatalogProp {
    pub key: String,
    pub value: String,
}

/// The resolved coordinates + secret needed to wire a Trino to egret's catalog.
#[derive(Clone, Debug)]
pub struct CatalogWiring {
    pub catalog_uri: String, // "http://egret:8080/catalog"
    pub token_uri: String,   // "http://egret:8080/oauth/token"
    pub warehouse: String,   // warehouse NAME
    pub s3_endpoint: String, // "http://minio:9000"
    pub client_id: String,
    pub client_secret: String,
}

/// Desired state of a Trino engine.
#[derive(Clone, Debug)]
pub struct EngineSpec {
    pub name: String, // DNS-1123 k8s name
    pub namespace: String,
    pub image_version: String, // e.g. "481"
    pub coordinator_replicas: u32,
    pub worker_replicas: u32,
    pub resources: Resources,
    pub wiring: CatalogWiring,
    pub extra_props: Vec<CatalogProp>,
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
