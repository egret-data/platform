//! Control-plane management for query engines (Trino on Kubernetes).
//!
//! Provisioning a Trino auto-wires it to egret's embedded Iceberg catalog: the
//! route resolves the warehouse + storage endpoint, mints a fresh Catalog Client
//! for the warehouse, and records that as the engine's PRIMARY catalog row. A
//! cluster can then carry additional catalogs (Postgres, tpch/tpcds, generic).
//!
//! The control-plane owns all connector-kind logic: [`build_catalog_req`]
//! translates each stored [`EngineCatalog`] row into a flat `{name, properties,
//! env}` [`CatalogReq`], and [`build_provision_req`] assembles the full
//! [`EngineProvisionReq`]. The actual k8s work lives behind the
//! [`EngineProvisioner`] trait, implemented in egret-bin over `egret-orchestrator`
//! so this crate never depends on kube (mirrors `WarehouseProvisioner`).

use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use axum::{Json, Router};
use egret_auth::principal::Auth;
use egret_catalog::service::embedded::{GrantInput, NewClient};
use egret_catalog::service::{Engine, EngineCatalog};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::require_platform_admin;
use crate::error::CpError;
use crate::CpState;

/// Default Trino image (official `trinodb/trino` tag) when the body omits it.
const DEFAULT_IMAGE_VERSION: &str = "470";
/// Kubernetes namespace engines are provisioned into (fixed for now).
const DEFAULT_NAMESPACE: &str = "egret-engines";
/// Shared per-warehouse role that engine Catalog Clients are bound to.
const ENGINE_ROLE_NAME: &str = "engine";
/// Catalog kinds the translation layer understands.
const CATALOG_KINDS: [&str; 5] = ["egret-warehouse", "postgresql", "tpch", "tpcds", "generic"];

/// One catalog rendered flat for the orchestrator (`<name>.properties` + env).
/// `properties` may contain `${ENV:VAR}` references resolved from `env`.
#[derive(Clone)]
pub struct CatalogReq {
    pub name: String,
    pub properties: BTreeMap<String, String>,
    pub env: BTreeMap<String, String>,
}

/// JVM heap sizing for a role: either a fixed size (`-Xmx`) or a RAM percentage.
/// Kube-free mirror of the orchestrator `HeapConfig`.
#[derive(Clone, Default)]
pub struct HeapReq {
    pub max_size: Option<String>,
    pub min_size: Option<String>,
    pub max_percent: Option<u8>,
    pub initial_percent: Option<u8>,
}

/// CPU/memory requests + limits for a role (mirrors orchestrator `RoleResources`).
#[derive(Clone, Default)]
pub struct RoleResourcesReq {
    pub cpu_request: Option<String>,
    pub cpu_limit: Option<String>,
    pub mem_request: Option<String>,
    pub mem_limit: Option<String>,
}

/// A single probe's tunable fields (mirrors orchestrator `Probe`).
#[derive(Clone, Default)]
pub struct ProbeReq {
    pub initial_delay_seconds: Option<u32>,
    pub period_seconds: Option<u32>,
    pub timeout_seconds: Option<u32>,
    pub failure_threshold: Option<u32>,
    pub success_threshold: Option<u32>,
}

/// The three probe kinds for a role (mirrors orchestrator `Probes`).
#[derive(Clone, Default)]
pub struct ProbesReq {
    pub liveness: Option<ProbeReq>,
    pub readiness: Option<ProbeReq>,
    pub startup: Option<ProbeReq>,
}

/// Per-role config shared by coordinator and worker (mirrors `RoleConfig`).
#[derive(Clone, Default)]
pub struct RoleConfigReq {
    pub replicas: u32,
    pub resources: RoleResourcesReq,
    pub heap: HeapReq,
    pub query_max_memory_per_node: Option<String>,
    pub heap_headroom_per_node: Option<String>,
    pub additional_jvm_config: Vec<String>,
    pub probes: ProbesReq,
}

/// One resource group in the file-based resource-group manager (recursive).
/// Kube-free mirror of the orchestrator `ResourceGroup`.
#[derive(Clone, Default)]
pub struct ResourceGroupReq {
    pub name: String,
    pub soft_memory_limit: String,
    pub hard_concurrency_limit: u32,
    pub max_queued: u32,
    pub scheduling_policy: Option<String>,
    pub sub_groups: Vec<ResourceGroupReq>,
}

/// A resource-group selector (mirrors orchestrator `ResourceSelector`).
#[derive(Clone, Default)]
pub struct ResourceSelectorReq {
    pub user: Option<String>,
    pub source: Option<String>,
    pub query_type: Option<String>,
    /// Dot-path into the group tree, e.g. `global.etl`.
    pub group: String,
}

/// The full resource-group config (mirrors orchestrator `ResourceGroups`).
#[derive(Clone, Default)]
pub struct ResourceGroupsReq {
    pub root_groups: Vec<ResourceGroupReq>,
    pub selectors: Vec<ResourceSelectorReq>,
}

/// One session-property match rule (mirrors orchestrator `SessionRule`).
#[derive(Clone, Default)]
pub struct SessionRuleReq {
    pub match_user: Option<String>,
    pub match_source: Option<String>,
    pub match_client_tags: Vec<String>,
    pub properties: BTreeMap<String, String>,
}

/// Fault-tolerant execution config with RESOLVED S3 exchange coordinates
/// (mirrors orchestrator `FaultTolerant`). The wire body carries
/// `{retryPolicy, exchangeStorageConnId, exchangePrefix}`; the handler resolves
/// the storage connection into these coords and passes the creds via
/// `TrinoConfigReq.env` (`EXCHANGE_S3_ACCESS_KEY` / `EXCHANGE_S3_SECRET_KEY`).
#[derive(Clone, Default)]
pub struct FaultTolerantReq {
    pub retry_policy: String,
    pub s3_base_dir: String,
    pub s3_endpoint: String,
    pub s3_region: String,
    pub s3_path_style: bool,
}

/// Worker graceful-shutdown drain settings (mirrors `GracefulShutdown`).
#[derive(Clone, Default)]
pub struct GracefulShutdownReq {
    pub enabled: bool,
    pub grace_period_seconds: u32,
}

/// Worker role: base role config plus optional graceful shutdown.
#[derive(Clone, Default)]
pub struct WorkerRoleConfigReq {
    pub base: RoleConfigReq,
    pub graceful_shutdown: Option<GracefulShutdownReq>,
}

/// One password-authenticator user (mirrors orchestrator `PasswordEntry`). Holds
/// the ALREADY-bcrypted hash: the control plane resolves the password secret and
/// hashes it in [`build_provision_req`], so the orchestrator never sees plaintext.
#[derive(Clone, Default)]
pub struct PasswordEntryReq {
    pub username: String,
    pub bcrypt_hash: String,
}

/// JWT authenticator config (mirrors orchestrator `Jwt`). `jwks_uri` defaults to
/// the control-plane's `engine_jwks_uri` when the wire value is empty/absent.
#[derive(Clone, Default)]
pub struct JwtReq {
    pub jwks_uri: String,
    pub required_audience: Option<String>,
}

/// Trino auth block: file-password users + an optional JWT authenticator
/// (mirrors orchestrator `Auth`).
#[derive(Clone, Default)]
pub struct AuthReq {
    pub password_users: Vec<PasswordEntryReq>,
    pub jwt: Option<JwtReq>,
}

/// Kubernetes Ingress exposure config (mirrors orchestrator `Ingress`). No
/// secrets, so it is parsed straight from the wire in [`trino_config_from_json`].
#[derive(Clone, Default)]
pub struct IngressReq {
    pub enabled: bool,
    pub class_name: Option<String>,
    pub host: String,
    pub tls_secret: Option<String>,
    pub annotations: BTreeMap<String, String>,
}

/// Fuller Trino config surface, split per-role, kube-free (mirrors the
/// orchestrator `TrinoConfig`).
#[derive(Clone, Default)]
pub struct TrinoConfigReq {
    pub query_max_memory: Option<String>,
    pub log_level: Option<String>,
    pub config_properties: BTreeMap<String, String>,
    pub env: BTreeMap<String, String>,
    pub coordinator: RoleConfigReq,
    pub worker: WorkerRoleConfigReq,
    pub additional_node_properties: BTreeMap<String, String>,
    pub additional_log_properties: BTreeMap<String, String>,
    pub event_listener: BTreeMap<String, String>,
    pub resource_groups: Option<ResourceGroupsReq>,
    pub session_properties: Vec<SessionRuleReq>,
    pub fault_tolerant: Option<FaultTolerantReq>,
    /// Password users (bcrypt hashes) + optional JWT. Resolved from the RAW wire
    /// `auth` (which carries `secretRef`, not hashes) in [`build_provision_req`].
    pub auth: Option<AuthReq>,
    pub ingress: Option<IngressReq>,
}

/// Resolved provisioning request handed to egret-bin (no k8s types here). The
/// route has already resolved every catalog into flat properties + env and the
/// config surface into [`TrinoConfigReq`] (which now carries per-role replicas +
/// resources, so no flat replica/resource fields live here anymore).
pub struct EngineProvisionReq {
    pub name: String,
    pub namespace: String,
    pub image_version: String,
    pub catalogs: Vec<CatalogReq>,
    pub config: TrinoConfigReq,
}

/// One live pod backing an engine.
pub struct EnginePod {
    pub name: String,
    pub phase: String,
    pub ready: bool,
}

/// Live engine status read back from the orchestrator + pods.
pub struct EngineLiveStatus {
    pub phase: String,
    pub message: Option<String>,
    pub pods: Vec<EnginePod>,
}

/// Provisions/updates/deletes a query engine on Kubernetes. Implemented in the
/// composition root (egret-bin) over `egret-orchestrator`; `None` in tests and
/// when no cluster is configured (routes then report "kubernetes not configured").
#[async_trait::async_trait]
pub trait EngineProvisioner: Send + Sync {
    async fn provision(&self, req: &EngineProvisionReq) -> Result<(), String>;
    async fn status(&self, namespace: &str, name: &str) -> Result<EngineLiveStatus, String>;
    async fn update(&self, req: &EngineProvisionReq) -> Result<(), String>;
    async fn delete(&self, namespace: &str, name: &str) -> Result<(), String>;

    /// Issue a raw HTTP request to a Trino coordinator `Service` and return
    /// `(status, body)`. Kube-free seam over `KubeClient::coordinator_request`
    /// (Service DNS in-cluster / kube API-server proxy in dev); non-2xx statuses
    /// are returned as-is for the caller to map.
    async fn coordinator_request(
        &self,
        namespace: &str,
        service: &str,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: Option<Vec<u8>>,
    ) -> anyhow::Result<(u16, Vec<u8>)>;
}

pub fn router() -> Router<CpState> {
    Router::new()
        .route("/engines", get(list_engines).post(create_engine))
        .route(
            "/engines/{id}",
            get(get_engine).put(update_engine).delete(delete_engine),
        )
        .route(
            "/engines/{id}/catalogs",
            get(list_catalogs).post(add_catalog),
        )
        .route("/engines/{id}/catalogs/{cid}", delete(remove_catalog))
        // Native Trino monitor (query list / detail / kill / cluster).
        .merge(crate::monitor::routes())
}

// ── DTOs (camelCase on the wire) ───────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateEngineBody {
    name: String,
    warehouse_id: Uuid,
    #[serde(default)]
    image_version: Option<String>,
    coordinator_replicas: u32,
    worker_replicas: u32,
    cpu_min: String,
    cpu_max: String,
    memory: String,
    /// Optional full per-role config surface, stored under `spec["trinoConfig"]`.
    /// Wire shape: `{ queryMaxMemory, logLevel, configProperties, env,
    /// coordinator{replicas, resources, heap, queryMaxMemoryPerNode,
    /// additionalJvmConfig, probes}, worker{... , gracefulShutdown} }`. On create
    /// the simple wizard fields seed this; `update` sends the full per-role block.
    #[serde(default)]
    trino_config: Option<serde_json::Value>,
}

/// The provisioning parameters surfaced back to clients (subset of the stored
/// `spec` jsonb; internal bookkeeping keys are ignored on deserialize).
#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct EngineSpecDto {
    coordinator_replicas: u32,
    worker_replicas: u32,
    cpu_min: String,
    cpu_max: String,
    memory: String,
    image_version: String,
    /// Surfaced back so the UI can prefill the config editor on reload.
    #[serde(skip_serializing_if = "Option::is_none")]
    trino_config: Option<serde_json::Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EngineDto {
    id: Uuid,
    name: String,
    #[serde(rename = "type")]
    r#type: String,
    namespace: String,
    warehouse_id: Uuid,
    status: String,
    spec: EngineSpecDto,
}
impl From<Engine> for EngineDto {
    fn from(e: Engine) -> Self {
        let spec = serde_json::from_value(e.spec).unwrap_or_default();
        EngineDto {
            id: e.id,
            name: e.name,
            r#type: e.r#type,
            namespace: e.namespace,
            warehouse_id: e.warehouse_id,
            status: e.status,
            spec,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EngineCatalogDto {
    id: Uuid,
    name: String,
    kind: String,
    is_primary: bool,
}
impl From<EngineCatalog> for EngineCatalogDto {
    fn from(c: EngineCatalog) -> Self {
        EngineCatalogDto {
            id: c.id,
            name: c.name,
            kind: c.kind,
            is_primary: c.is_primary,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddCatalogBody {
    name: String,
    kind: String,
    #[serde(default)]
    config: serde_json::Value,
    #[serde(default)]
    secret_ref: Option<Uuid>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EnginePodDto {
    name: String,
    phase: String,
    ready: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EngineStatusDto {
    phase: String,
    message: Option<String>,
    pods: Vec<EnginePodDto>,
}

/// `GET /engines/{id}` payload: the engine row flattened with live status.
#[derive(Serialize)]
struct EngineDetailDto {
    #[serde(flatten)]
    engine: EngineDto,
    status: EngineStatusDto,
}

// ── translation: stored catalog rows → provisioning request ─────────────────────

/// Translate one stored catalog row into a flat Trino catalog (properties + env).
/// `env_prefix` namespaces this catalog's secret env vars (e.g. "C0") so two
/// catalogs never collide on an env key.
async fn build_catalog_req(
    st: &CpState,
    _engine: &Engine,
    cat: &EngineCatalog,
    env_prefix: &str,
) -> Result<CatalogReq, CpError> {
    let mut props = BTreeMap::new();
    let mut env = BTreeMap::new();
    match cat.kind.as_str() {
        "egret-warehouse" => {
            // Warehouse name + a minted client live in cat.config; the OAuth
            // credential is passed via env so it never lands in the ConfigMap.
            let wh_name = cat.config["warehouse"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let cred_var = format!("EGRET_{env_prefix}");
            props.insert("connector.name".into(), "iceberg".into());
            props.insert("iceberg.catalog.type".into(), "rest".into());
            props.insert(
                "iceberg.rest-catalog.uri".into(),
                format!("{}/catalog", st.engine_egret_url),
            );
            props.insert("iceberg.rest-catalog.warehouse".into(), wh_name);
            props.insert("iceberg.rest-catalog.security".into(), "OAUTH2".into());
            props.insert(
                "iceberg.rest-catalog.oauth2.server-uri".into(),
                format!("{}/oauth/token", st.engine_egret_url),
            );
            props.insert(
                "iceberg.rest-catalog.oauth2.credential".into(),
                format!("${{ENV:{cred_var}}}"),
            );
            props.insert(
                "iceberg.rest-catalog.vended-credentials-enabled".into(),
                "true".into(),
            );
            props.insert("fs.native-s3.enabled".into(), "true".into());
            props.insert(
                "s3.endpoint".into(),
                cat.config["s3_endpoint"]
                    .as_str()
                    .unwrap_or_default()
                    .into(),
            );
            props.insert("s3.region".into(), "us-east-1".into());
            props.insert("s3.path-style-access".into(), "true".into());
            let cred = cat.config["credential"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            env.insert(cred_var, cred);
        }
        "postgresql" => {
            props.insert("connector.name".into(), "postgresql".into());
            props.insert(
                "connection-url".into(),
                format!(
                    "jdbc:postgresql://{}:{}/{}",
                    cat.config["host"].as_str().unwrap_or_default(),
                    cat.config["port"].as_i64().unwrap_or(5432),
                    cat.config["database"].as_str().unwrap_or_default()
                ),
            );
            props.insert(
                "connection-user".into(),
                cat.config["user"].as_str().unwrap_or_default().into(),
            );
            let pw_var = format!("PG_{env_prefix}");
            props.insert("connection-password".into(), format!("${{ENV:{pw_var}}}"));
            if let Some(ref_id) = cat.secret_ref {
                let pw = st
                    .secrets
                    .resolve_plaintext(None, ref_id)
                    .await
                    .map_err(|e| CpError::BadRequest(format!("resolve secret: {e}")))?;
                env.insert(pw_var, pw);
            }
        }
        "tpch" => {
            props.insert("connector.name".into(), "tpch".into());
        }
        "tpcds" => {
            props.insert("connector.name".into(), "tpcds".into());
        }
        "generic" => {
            props.insert(
                "connector.name".into(),
                cat.config["connectorName"]
                    .as_str()
                    .unwrap_or_default()
                    .into(),
            );
            if let Some(obj) = cat.config["properties"].as_object() {
                for (k, v) in obj {
                    props.insert(k.clone(), v.as_str().unwrap_or_default().into());
                }
            }
        }
        other => {
            return Err(CpError::BadRequest(format!(
                "unknown catalog kind: {other}"
            )))
        }
    }
    Ok(CatalogReq {
        name: cat.name.clone(),
        properties: props,
        env,
    })
}

/// Build the full provisioning request from the engine + its stored catalogs + config.
async fn build_provision_req(st: &CpState, engine: &Engine) -> Result<EngineProvisionReq, CpError> {
    let cats = st.storage.list_engine_catalogs(engine.id).await?;
    let mut catalogs = Vec::new();
    for (i, c) in cats.iter().enumerate() {
        catalogs.push(build_catalog_req(st, engine, c, &format!("C{i}")).await?);
    }
    let mut config = build_trino_config(&engine.spec);
    // Resolve the fault-tolerant exchange storage connection (if any) into S3
    // coords + creds, mirroring the egret-warehouse arm's storage resolution.
    config.fault_tolerant =
        resolve_fault_tolerant(st, &engine.spec["trinoConfig"], &mut config.env).await?;
    // Resolve the RAW auth block (password secrets → bcrypt hashes, JWKS default),
    // mirroring the fault-tolerant storage resolution. Ingress is parsed inline.
    config.auth = resolve_auth(st, &engine.spec["trinoConfig"]).await?;
    // When the engine ALREADY has auth on, inject the derived `egret-monitor`
    // system user so the native Trino monitor can authenticate. Guarded on
    // `auth.is_some()` (inside the helper) so a no-auth engine stays auth-free.
    inject_monitor_user(&mut config.auth, engine.id)?;
    // Trino requires a coordinator<->worker shared secret whenever auth is enabled.
    // Derive a stable per-engine value (both roles read it from the env Secret).
    if config.auth.is_some() {
        config.env.insert(
            "TRINO_INTERNAL_SECRET".into(),
            format!("egret-internal-{}", engine.id),
        );
    }
    Ok(EngineProvisionReq {
        name: engine.name.clone(),
        namespace: engine.namespace.clone(),
        image_version: engine.spec["imageVersion"]
            .as_str()
            .unwrap_or(DEFAULT_IMAGE_VERSION)
            .into(),
        catalogs,
        config,
    })
}

/// Resolve the stored `trinoConfig.faultTolerant`
/// `{retryPolicy, exchangeStorageConnId, exchangePrefix}` into a
/// [`FaultTolerantReq`] with resolved S3 coords, pushing the exchange creds into
/// `env` as `EXCHANGE_S3_ACCESS_KEY` / `EXCHANGE_S3_SECRET_KEY`. `None` when the
/// block is absent; `BadRequest` when the connection or its secret is missing.
async fn resolve_fault_tolerant(
    st: &CpState,
    tc: &serde_json::Value,
    env: &mut BTreeMap<String, String>,
) -> Result<Option<FaultTolerantReq>, CpError> {
    let ft = &tc["faultTolerant"];
    if !ft.is_object() {
        return Ok(None);
    }
    let Some(conn_id) = ft["exchangeStorageConnId"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
    else {
        return Ok(None);
    };
    let sc = st
        .storage
        .get_storage_conn(conn_id)
        .await?
        .ok_or_else(|| CpError::BadRequest("unknown exchange storage connection".into()))?;
    let secret_ref = sc
        .secret_ref
        .ok_or_else(|| CpError::BadRequest("exchange storage connection has no secret".into()))?;
    let secret_key = st
        .secrets
        .resolve_plaintext(None, secret_ref)
        .await
        .map_err(|e| CpError::BadRequest(format!("resolve exchange secret: {e}")))?;
    env.insert("EXCHANGE_S3_ACCESS_KEY".into(), sc.access_key.clone());
    env.insert("EXCHANGE_S3_SECRET_KEY".into(), secret_key);
    let prefix = ft["exchangePrefix"].as_str().unwrap_or_default();
    Ok(Some(FaultTolerantReq {
        retry_policy: ft["retryPolicy"].as_str().unwrap_or("TASK").to_string(),
        s3_base_dir: format!("s3://{}/{}", sc.bucket, prefix),
        s3_endpoint: sc.endpoint.clone().unwrap_or_default(),
        s3_region: sc.region.clone(),
        s3_path_style: sc.path_style,
    }))
}

/// Collect a `{ "k": "v", ... }` JSON object into a `BTreeMap<String, String>`
/// (non-string values are dropped).
fn str_map(v: &serde_json::Value) -> BTreeMap<String, String> {
    v.as_object()
        .map(|o| {
            o.iter()
                .filter_map(|(k, val)| val.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn parse_heap(v: &serde_json::Value) -> HeapReq {
    HeapReq {
        max_size: v["maxSize"].as_str().map(Into::into),
        min_size: v["minSize"].as_str().map(Into::into),
        max_percent: v["maxPercent"].as_u64().map(|n| n as u8),
        initial_percent: v["initialPercent"].as_u64().map(|n| n as u8),
    }
}

fn parse_resources(v: &serde_json::Value) -> RoleResourcesReq {
    RoleResourcesReq {
        cpu_request: v["cpuRequest"].as_str().map(Into::into),
        cpu_limit: v["cpuLimit"].as_str().map(Into::into),
        mem_request: v["memRequest"].as_str().map(Into::into),
        mem_limit: v["memLimit"].as_str().map(Into::into),
    }
}

/// A probe carrier only when the wire object is present (so an unset probe stays
/// `None` and the orchestrator applies its default).
fn parse_probe(v: &serde_json::Value) -> Option<ProbeReq> {
    if !v.is_object() {
        return None;
    }
    Some(ProbeReq {
        initial_delay_seconds: v["initialDelaySeconds"].as_u64().map(|n| n as u32),
        period_seconds: v["periodSeconds"].as_u64().map(|n| n as u32),
        timeout_seconds: v["timeoutSeconds"].as_u64().map(|n| n as u32),
        failure_threshold: v["failureThreshold"].as_u64().map(|n| n as u32),
        success_threshold: v["successThreshold"].as_u64().map(|n| n as u32),
    })
}

fn parse_probes(v: &serde_json::Value) -> ProbesReq {
    ProbesReq {
        liveness: parse_probe(&v["liveness"]),
        readiness: parse_probe(&v["readiness"]),
        startup: parse_probe(&v["startup"]),
    }
}

fn parse_role(v: &serde_json::Value, default_replicas: u32) -> RoleConfigReq {
    RoleConfigReq {
        replicas: v["replicas"]
            .as_u64()
            .map(|n| n as u32)
            .unwrap_or(default_replicas),
        resources: parse_resources(&v["resources"]),
        heap: parse_heap(&v["heap"]),
        query_max_memory_per_node: v["queryMaxMemoryPerNode"].as_str().map(Into::into),
        heap_headroom_per_node: v["heapHeadroomPerNode"].as_str().map(Into::into),
        additional_jvm_config: v["additionalJvmConfig"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        probes: parse_probes(&v["probes"]),
    }
}

/// Read `engine.spec["trinoConfig"]`. If it already carries a per-role
/// `coordinator` block (Phase 1.5.1 shape), parse it directly; otherwise
/// synthesise the per-role config from the old flat Phase-1.5 spec fields
/// (back-compat for engines provisioned before this reshape).
fn build_trino_config(spec: &serde_json::Value) -> TrinoConfigReq {
    let tc = &spec["trinoConfig"];
    if tc["coordinator"].is_object() {
        return trino_config_from_json(tc);
    }
    // ── back-compat: old flat spec ──────────────────────────────────────────
    let resources = RoleResourcesReq {
        cpu_request: spec["cpuMin"].as_str().map(Into::into),
        cpu_limit: spec["cpuMax"].as_str().map(Into::into),
        mem_request: spec["memory"].as_str().map(Into::into),
        mem_limit: spec["memory"].as_str().map(Into::into),
    };
    let heap = HeapReq {
        max_size: tc["jvmHeap"].as_str().map(Into::into),
        ..Default::default()
    };
    let qmpn = tc["queryMaxMemoryPerNode"].as_str().map(String::from);
    let coordinator = RoleConfigReq {
        replicas: spec["coordinatorReplicas"].as_u64().unwrap_or(1) as u32,
        resources: resources.clone(),
        heap: heap.clone(),
        query_max_memory_per_node: qmpn.clone(),
        ..Default::default()
    };
    let worker_base = RoleConfigReq {
        replicas: spec["workerReplicas"].as_u64().unwrap_or(1) as u32,
        resources,
        heap,
        query_max_memory_per_node: qmpn,
        ..Default::default()
    };
    TrinoConfigReq {
        query_max_memory: tc["queryMaxMemory"].as_str().map(Into::into),
        log_level: None,
        config_properties: str_map(&tc["configProperties"]),
        env: str_map(&tc["env"]),
        coordinator,
        worker: WorkerRoleConfigReq {
            base: worker_base,
            graceful_shutdown: None,
        },
        ..Default::default()
    }
}

/// Seed a per-role `trinoConfig` wire value from the simple create wizard: both
/// roles get the same cpuMin/cpuMax/memory resources; replicas come from the body.
fn seed_trino_config(b: &CreateEngineBody) -> serde_json::Value {
    let resources = serde_json::json!({
        "cpuRequest": b.cpu_min,
        "cpuLimit": b.cpu_max,
        "memRequest": b.memory,
        "memLimit": b.memory,
    });
    serde_json::json!({
        "coordinator": { "replicas": b.coordinator_replicas, "resources": resources },
        "worker": { "replicas": b.worker_replicas, "resources": resources },
    })
}

/// Parse one recursive resource-group node from the camelCase wire shape.
fn parse_resource_group(v: &serde_json::Value) -> ResourceGroupReq {
    ResourceGroupReq {
        name: v["name"].as_str().unwrap_or_default().to_string(),
        soft_memory_limit: v["softMemoryLimit"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        hard_concurrency_limit: v["hardConcurrencyLimit"].as_u64().unwrap_or(0) as u32,
        max_queued: v["maxQueued"].as_u64().unwrap_or(0) as u32,
        scheduling_policy: v["schedulingPolicy"].as_str().map(Into::into),
        sub_groups: v["subGroups"]
            .as_array()
            .map(|a| a.iter().map(parse_resource_group).collect())
            .unwrap_or_default(),
    }
}

/// Parse the `resourceGroups{rootGroups,selectors}` wire object; `None` when absent.
fn parse_resource_groups(v: &serde_json::Value) -> Option<ResourceGroupsReq> {
    if !v.is_object() {
        return None;
    }
    let selectors = v["selectors"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|s| ResourceSelectorReq {
                    user: s["user"].as_str().map(Into::into),
                    source: s["source"].as_str().map(Into::into),
                    query_type: s["queryType"].as_str().map(Into::into),
                    group: s["group"].as_str().unwrap_or_default().to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    Some(ResourceGroupsReq {
        root_groups: v["rootGroups"]
            .as_array()
            .map(|a| a.iter().map(parse_resource_group).collect())
            .unwrap_or_default(),
        selectors,
    })
}

/// Parse the `sessionProperties[]` wire array into session-rule carriers.
fn parse_session_rules(v: &serde_json::Value) -> Vec<SessionRuleReq> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|r| SessionRuleReq {
                    match_user: r["matchUser"].as_str().map(Into::into),
                    match_source: r["matchSource"].as_str().map(Into::into),
                    match_client_tags: r["matchClientTags"]
                        .as_array()
                        .map(|t| {
                            t.iter()
                                .filter_map(|x| x.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default(),
                    properties: str_map(&r["properties"]),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every selector's group first segment (before the first `.`) must name an
/// existing root group; a dangling reference is a `BadRequest`.
fn validate_resource_groups(rg: &ResourceGroupsReq) -> Result<(), CpError> {
    let roots: std::collections::HashSet<&str> =
        rg.root_groups.iter().map(|g| g.name.as_str()).collect();
    for s in &rg.selectors {
        let root = s.group.split('.').next().unwrap_or_default();
        if !roots.contains(root) {
            return Err(CpError::BadRequest(format!(
                "resource-group selector references unknown root group: {root:?}"
            )));
        }
    }
    Ok(())
}

/// Validate any `resourceGroups` block carried on a spec's `trinoConfig` (no-op
/// when absent) before it is persisted/provisioned.
fn validate_spec_resource_groups(spec: &serde_json::Value) -> Result<(), CpError> {
    if let Some(rg) = parse_resource_groups(&spec["trinoConfig"]["resourceGroups"]) {
        validate_resource_groups(&rg)?;
    }
    Ok(())
}

/// Parse the nested per-role `trinoConfig` wire shape (camelCase) into carriers.
/// NOTE: `faultTolerant` is NOT parsed here — its wire body carries a storage
/// connection id + prefix (not resolved S3 coords), so it is resolved later in
/// [`build_provision_req`].
fn trino_config_from_json(v: &serde_json::Value) -> TrinoConfigReq {
    let worker_v = &v["worker"];
    let graceful_shutdown = {
        let g = &worker_v["gracefulShutdown"];
        g.is_object().then(|| GracefulShutdownReq {
            enabled: g["enabled"].as_bool().unwrap_or(false),
            grace_period_seconds: g["gracePeriodSeconds"].as_u64().unwrap_or(0) as u32,
        })
    };
    TrinoConfigReq {
        query_max_memory: v["queryMaxMemory"].as_str().map(Into::into),
        log_level: v["logLevel"].as_str().map(Into::into),
        config_properties: str_map(&v["configProperties"]),
        env: str_map(&v["env"]),
        coordinator: parse_role(&v["coordinator"], 1),
        worker: WorkerRoleConfigReq {
            base: parse_role(worker_v, 1),
            graceful_shutdown,
        },
        additional_node_properties: str_map(&v["additionalNodeProperties"]),
        additional_log_properties: str_map(&v["additionalLogProperties"]),
        event_listener: str_map(&v["eventListener"]),
        resource_groups: parse_resource_groups(&v["resourceGroups"]),
        session_properties: parse_session_rules(&v["sessionProperties"]),
        // Resolved in build_provision_req from the raw {retryPolicy,
        // exchangeStorageConnId, exchangePrefix} wire body.
        fault_tolerant: None,
        // Resolved in build_provision_req from the raw {passwordUsers:[{username,
        // secretRef}], jwt} wire body (secret → bcrypt). Ingress has no secrets.
        auth: None,
        ingress: parse_ingress(&v["ingress"]),
    }
}

/// Parse the `ingress { enabled, className?, host, tlsSecret?, annotations? }`
/// wire object into an [`IngressReq`]; `None` when absent.
fn parse_ingress(v: &serde_json::Value) -> Option<IngressReq> {
    if !v.is_object() {
        return None;
    }
    Some(IngressReq {
        enabled: v["enabled"].as_bool().unwrap_or(false),
        class_name: v["className"].as_str().map(Into::into),
        host: v["host"].as_str().unwrap_or_default().to_string(),
        tls_secret: v["tlsSecret"].as_str().map(Into::into),
        annotations: str_map(&v["annotations"]),
    })
}

/// Resolve the RAW `trinoConfig.auth`
/// `{ passwordUsers: [{username, secretRef}], jwt?: {jwksUri?, requiredAudience?} }`
/// into an [`AuthReq`]: each password secret is resolved + bcrypted so the
/// orchestrator never sees plaintext, and an empty `jwksUri` falls back to the
/// control-plane default [`CpState::engine_jwks_uri`]. `None` when the block is
/// absent; `BadRequest` when a referenced secret is missing/unresolvable.
async fn resolve_auth(st: &CpState, tc: &serde_json::Value) -> Result<Option<AuthReq>, CpError> {
    let auth = &tc["auth"];
    if !auth.is_object() {
        return Ok(None);
    }
    let mut password_users = Vec::new();
    if let Some(arr) = auth["passwordUsers"].as_array() {
        for u in arr {
            let username = u["username"].as_str().unwrap_or_default().to_string();
            let Some(secret_ref) = u["secretRef"]
                .as_str()
                .and_then(|s| Uuid::parse_str(s).ok())
            else {
                return Err(CpError::BadRequest(
                    "password user missing or invalid secretRef".into(),
                ));
            };
            let pw = st
                .secrets
                .resolve_plaintext(None, secret_ref)
                .await
                .map_err(|e| CpError::BadRequest(format!("resolve password secret: {e}")))?;
            // Trino's file password authenticator (jBCrypt) accepts the `$2y$`
            // variant; the bcrypt crate emits `$2b$`. They are algorithmically
            // identical, so relabel the prefix.
            let bcrypt_hash = bcrypt::hash(pw, bcrypt::DEFAULT_COST)
                .map_err(|e| CpError::BadRequest(format!("bcrypt password: {e}")))?
                .replacen("$2b$", "$2y$", 1);
            password_users.push(PasswordEntryReq {
                username,
                bcrypt_hash,
            });
        }
    }
    let jwt = {
        let j = &auth["jwt"];
        j.is_object().then(|| {
            let jwks_uri = j["jwksUri"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(String::from)
                .unwrap_or_else(|| st.engine_jwks_uri.clone());
            JwtReq {
                jwks_uri,
                required_audience: j["requiredAudience"].as_str().map(Into::into),
            }
        })
    };
    Ok(Some(AuthReq {
        password_users,
        jwt,
    }))
}

/// System user the native Trino monitor (Phase 1.6b) authenticates as against a
/// coordinator that has auth enabled. The password is DERIVED from the engine id
/// (no storage): both provisioning (bcrypt into `password.db`, [`inject_monitor_user`])
/// and the monitor's Basic auth header re-derive the same value.
pub fn monitor_password(id: Uuid) -> String {
    format!("egret-monitor-{id}")
}

/// Inject the derived `egret-monitor` password entry into an engine's auth block
/// so the native monitor can authenticate. No-op when `auth` is `None`: this must
/// NOT turn auth on for engines that have none — it only rides along when auth is
/// already enabled. Uses the same `$2b$`→`$2y$` relabel as the human users so
/// Trino's jBCrypt file authenticator accepts the hash.
fn inject_monitor_user(auth: &mut Option<AuthReq>, engine_id: Uuid) -> Result<(), CpError> {
    let Some(auth) = auth.as_mut() else {
        return Ok(());
    };
    let bcrypt_hash = bcrypt::hash(monitor_password(engine_id), bcrypt::DEFAULT_COST)
        .map_err(|e| CpError::BadRequest(format!("bcrypt monitor password: {e}")))?
        .replacen("$2b$", "$2y$", 1);
    auth.password_users.push(PasswordEntryReq {
        username: "egret-monitor".into(),
        bcrypt_hash,
    });
    Ok(())
}

// ── handlers ───────────────────────────────────────────────────────────────────

async fn list_engines(
    State(st): State<CpState>,
    Auth(p): Auth,
) -> Result<Json<Vec<EngineDto>>, CpError> {
    require_platform_admin(&p)?;
    Ok(Json(
        st.storage
            .list_engines()
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
    ))
}

/// Find (or create) the shared per-warehouse engine role and return its id. A new
/// role is granted catalog-wide `manage` so the engine can read + write the
/// whole warehouse.
async fn ensure_engine_role(st: &CpState, warehouse_id: Uuid) -> Result<Uuid, CpError> {
    if let Some(r) = st
        .storage
        .list_roles(warehouse_id)
        .await?
        .into_iter()
        .find(|r| r.name == ENGINE_ROLE_NAME)
    {
        return Ok(r.id);
    }
    let role = st
        .storage
        .create_role(
            warehouse_id,
            ENGINE_ROLE_NAME,
            "engine",
            Some("Auto-created role for query engines"),
        )
        .await?;
    st.storage
        .add_grant(
            role.id,
            &GrantInput {
                securable_type: "catalog".into(),
                namespace: None,
                table_name: None,
                privilege: "manage".into(),
            },
        )
        .await?;
    Ok(role.id)
}

/// Extract the internal Catalog Client row id stashed in a catalog's `config`.
fn client_row_id(config: &serde_json::Value) -> Option<Uuid> {
    config
        .get("clientRowId")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
}

async fn create_engine(
    State(st): State<CpState>,
    Auth(p): Auth,
    Json(b): Json<CreateEngineBody>,
) -> Result<(StatusCode, Json<EngineDto>), CpError> {
    require_platform_admin(&p)?;

    // 1. Resolve the warehouse; it must exist and be provisioned (has a
    //    lakekeeper counterpart) — engines connect by warehouse NAME.
    let wh = st
        .storage
        .get_warehouse(b.warehouse_id)
        .await?
        .ok_or_else(|| CpError::BadRequest("unknown warehouse".into()))?;
    if wh.lakekeeper_warehouse_id.is_none() {
        return Err(CpError::BadRequest(
            "warehouse is not provisioned yet".into(),
        ));
    }

    // 2. Resolve the storage endpoint for the Trino S3 connector.
    let sc = st
        .storage
        .get_storage_conn(wh.storage_conn_id)
        .await?
        .ok_or_else(|| CpError::BadRequest("unknown storage connection".into()))?;
    let s3_endpoint = sc.endpoint.unwrap_or_default();

    // 3. Auto-wire: mint a fresh Catalog Client for the warehouse.
    let role_id = ensure_engine_role(&st, wh.id).await?;
    let client = st
        .storage
        .create_client(&NewClient {
            name: format!("engine-{}", b.name),
            consumer_type: "engine".into(),
            workspace_id: wh.workspace_id,
            warehouse_id: wh.id,
            catalog_role_id: role_id,
        })
        .await?;

    // 4. Record the intent row (status `provisioning`). The auto-wired client id
    //    lives on the PRIMARY catalog row now, not in the engine spec.
    let image_version = b
        .image_version
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_IMAGE_VERSION.into());
    let namespace = DEFAULT_NAMESPACE.to_string();
    let mut spec = serde_json::json!({
        "coordinatorReplicas": b.coordinator_replicas,
        "workerReplicas": b.worker_replicas,
        "cpuMin": b.cpu_min,
        "cpuMax": b.cpu_max,
        "memory": b.memory,
        "imageVersion": image_version,
    });
    // Seed the per-role trinoConfig from the wizard inputs (both roles share the
    // cpuMin/cpuMax/memory resources; replicas come from the body). Any
    // per-role block supplied in the body overlays these seeded defaults.
    let mut trino_config = seed_trino_config(&b);
    if let (Some(seed), Some(supplied)) = (
        trino_config.as_object_mut(),
        b.trino_config.as_ref().and_then(|v| v.as_object()),
    ) {
        for (k, v) in supplied {
            seed.insert(k.clone(), v.clone());
        }
    }
    spec["trinoConfig"] = trino_config;
    validate_spec_resource_groups(&spec)?;
    let engine = match st
        .storage
        .create_engine(&b.name, "trino", &namespace, wh.id, &spec)
        .await
    {
        Ok(e) => e,
        Err(e) => {
            let _ = st.storage.revoke_client(client.id).await;
            return Err(e.into());
        }
    };

    // 5. Record the PRIMARY catalog row (the auto-wired egret warehouse).
    let cat_config = serde_json::json!({
        "warehouse": wh.name,
        "s3_endpoint": s3_endpoint,
        "credential": format!("{}:{}", client.client_id, client.client_secret),
        "clientRowId": client.id,
    });
    if let Err(e) = st
        .storage
        .create_engine_catalog(
            engine.id,
            "egret",
            "egret-warehouse",
            &cat_config,
            None,
            true,
        )
        .await
    {
        let _ = st.storage.delete_engine(engine.id).await;
        let _ = st.storage.revoke_client(client.id).await;
        return Err(e.into());
    }

    // 6. Build the provisioning request from the stored engine + catalogs.
    let req = match build_provision_req(&st, &engine).await {
        Ok(r) => r,
        Err(e) => {
            let _ = st.storage.delete_engine(engine.id).await; // CASCADE removes the catalog
            let _ = st.storage.revoke_client(client.id).await;
            return Err(e);
        }
    };

    // 7. Provision on Kubernetes; roll back the engine row (+ cascade catalog) +
    //    client on any failure.
    let Some(prov) = st.engine_provisioner.as_ref() else {
        let _ = st.storage.delete_engine(engine.id).await;
        let _ = st.storage.revoke_client(client.id).await;
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };
    if let Err(msg) = prov.provision(&req).await {
        let _ = st.storage.delete_engine(engine.id).await;
        let _ = st.storage.revoke_client(client.id).await;
        return Err(CpError::BadRequest(format!(
            "engine provisioning failed: {msg}"
        )));
    }

    Ok((StatusCode::CREATED, Json(engine.into())))
}

async fn get_engine(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<EngineDetailDto>, CpError> {
    require_platform_admin(&p)?;
    let engine = st.storage.get_engine(id).await?.ok_or(CpError::NotFound)?;
    let status = match st.engine_provisioner.as_ref() {
        Some(prov) => match prov.status(&engine.namespace, &engine.name).await {
            Ok(s) => EngineStatusDto {
                phase: s.phase,
                message: s.message,
                pods: s
                    .pods
                    .into_iter()
                    .map(|p| EnginePodDto {
                        name: p.name,
                        phase: p.phase,
                        ready: p.ready,
                    })
                    .collect(),
            },
            // A live-status read failure shouldn't hide the engine; fall back to
            // the recorded row status with the error as a message.
            Err(msg) => EngineStatusDto {
                phase: engine.status.clone(),
                message: Some(msg),
                pods: vec![],
            },
        },
        None => EngineStatusDto {
            phase: engine.status.clone(),
            message: None,
            pods: vec![],
        },
    };
    Ok(Json(EngineDetailDto {
        engine: engine.into(),
        status,
    }))
}

async fn update_engine(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(b): Json<CreateEngineBody>,
) -> Result<Json<EngineDto>, CpError> {
    require_platform_admin(&p)?;
    let engine = st.storage.get_engine(id).await?.ok_or(CpError::NotFound)?;

    // Rebuild the spec from the body + config. The primary Catalog Client is NOT
    // re-minted (its credential lives on the primary catalog row), so nothing is
    // revoked here. Trino has no hot-reload; the orchestrator rolls the pods.
    let image_version = b
        .image_version
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_IMAGE_VERSION.into());
    let mut spec = serde_json::json!({
        "coordinatorReplicas": b.coordinator_replicas,
        "workerReplicas": b.worker_replicas,
        "cpuMin": b.cpu_min,
        "cpuMax": b.cpu_max,
        "memory": b.memory,
        "imageVersion": image_version,
    });
    if let Some(tc) = &b.trino_config {
        spec["trinoConfig"] = tc.clone();
    } else if let Some(tc) = engine.spec.get("trinoConfig") {
        spec["trinoConfig"] = tc.clone();
    }
    validate_spec_resource_groups(&spec)?;
    st.storage.update_engine(id, &spec, &engine.status).await?;
    let engine = st.storage.get_engine(id).await?.ok_or(CpError::NotFound)?;

    let req = build_provision_req(&st, &engine).await?;
    let Some(prov) = st.engine_provisioner.as_ref() else {
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };
    if let Err(msg) = prov.update(&req).await {
        return Err(CpError::BadRequest(format!("engine update failed: {msg}")));
    }
    Ok(Json(engine.into()))
}

async fn delete_engine(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, CpError> {
    require_platform_admin(&p)?;
    let engine = st.storage.get_engine(id).await?.ok_or(CpError::NotFound)?;
    // Collect any minted Catalog Clients referenced by the catalog rows before the
    // engine (and its cascading catalogs) are removed.
    let cats = st
        .storage
        .list_engine_catalogs(engine.id)
        .await
        .unwrap_or_default();
    if let Some(prov) = st.engine_provisioner.as_ref() {
        prov.delete(&engine.namespace, &engine.name)
            .await
            .map_err(|e| CpError::BadRequest(format!("engine delete failed: {e}")))?;
    }
    st.storage.delete_engine(id).await?; // CASCADE removes engine_catalogs
    for c in cats {
        if let Some(cid) = client_row_id(&c.config) {
            let _ = st.storage.revoke_client(cid).await;
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

// ── catalog sub-routes ───────────────────────────────────────────────────────────

async fn list_catalogs(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<EngineCatalogDto>>, CpError> {
    require_platform_admin(&p)?;
    let _engine = st.storage.get_engine(id).await?.ok_or(CpError::NotFound)?;
    Ok(Json(
        st.storage
            .list_engine_catalogs(id)
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
    ))
}

async fn add_catalog(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(b): Json<AddCatalogBody>,
) -> Result<(StatusCode, Json<EngineCatalogDto>), CpError> {
    require_platform_admin(&p)?;
    let engine = st.storage.get_engine(id).await?.ok_or(CpError::NotFound)?;
    if !CATALOG_KINDS.contains(&b.kind.as_str()) {
        return Err(CpError::BadRequest(format!(
            "unknown catalog kind: {}",
            b.kind
        )));
    }

    // For an egret-warehouse catalog, resolve the warehouse + endpoint and mint a
    // fresh client (same as the primary); other kinds store the body as-is.
    let (config, secret_ref, minted_client): (serde_json::Value, Option<Uuid>, Option<Uuid>) = if b
        .kind
        == "egret-warehouse"
    {
        let wh_id = b.config["warehouseId"]
            .as_str()
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| {
                CpError::BadRequest("egret-warehouse catalog requires config.warehouseId".into())
            })?;
        let wh = st
            .storage
            .get_warehouse(wh_id)
            .await?
            .ok_or_else(|| CpError::BadRequest("unknown warehouse".into()))?;
        if wh.lakekeeper_warehouse_id.is_none() {
            return Err(CpError::BadRequest(
                "warehouse is not provisioned yet".into(),
            ));
        }
        let sc = st
            .storage
            .get_storage_conn(wh.storage_conn_id)
            .await?
            .ok_or_else(|| CpError::BadRequest("unknown storage connection".into()))?;
        let s3_endpoint = sc.endpoint.unwrap_or_default();
        let role_id = ensure_engine_role(&st, wh.id).await?;
        let client = st
            .storage
            .create_client(&NewClient {
                name: format!("engine-{}-{}", engine.name, b.name),
                consumer_type: "engine".into(),
                workspace_id: wh.workspace_id,
                warehouse_id: wh.id,
                catalog_role_id: role_id,
            })
            .await?;
        let cfg = serde_json::json!({
            "warehouse": wh.name,
            "s3_endpoint": s3_endpoint,
            "credential": format!("{}:{}", client.client_id, client.client_secret),
            "clientRowId": client.id,
        });
        (cfg, None, Some(client.id))
    } else {
        (b.config.clone(), b.secret_ref, None)
    };

    let cat = match st
        .storage
        .create_engine_catalog(engine.id, &b.name, &b.kind, &config, secret_ref, false)
        .await
    {
        Ok(c) => c,
        Err(e) => {
            if let Some(cid) = minted_client {
                let _ = st.storage.revoke_client(cid).await;
            }
            return Err(e.into());
        }
    };

    // Re-render + roll. On failure, undo the row (+ client).
    let rollback = |st: CpState, cat_id: Uuid, minted: Option<Uuid>| async move {
        let _ = st.storage.delete_engine_catalog(cat_id).await;
        if let Some(cid) = minted {
            let _ = st.storage.revoke_client(cid).await;
        }
    };
    let req = match build_provision_req(&st, &engine).await {
        Ok(r) => r,
        Err(e) => {
            rollback(st.clone(), cat.id, minted_client).await;
            return Err(e);
        }
    };
    let Some(prov) = st.engine_provisioner.as_ref() else {
        rollback(st.clone(), cat.id, minted_client).await;
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };
    if let Err(msg) = prov.update(&req).await {
        rollback(st.clone(), cat.id, minted_client).await;
        return Err(CpError::BadRequest(format!("engine update failed: {msg}")));
    }

    Ok((StatusCode::CREATED, Json(cat.into())))
}

async fn remove_catalog(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, cid)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, CpError> {
    require_platform_admin(&p)?;
    let engine = st.storage.get_engine(id).await?.ok_or(CpError::NotFound)?;
    let cat = st
        .storage
        .get_engine_catalog(cid)
        .await?
        .ok_or(CpError::NotFound)?;
    // Guard against cross-engine deletes via a mismatched path.
    if cat.engine_id != engine.id {
        return Err(CpError::NotFound);
    }
    if cat.is_primary {
        return Err(CpError::BadRequest(
            "cannot delete the primary catalog".into(),
        ));
    }
    st.storage.delete_engine_catalog(cat.id).await?;
    // Best-effort: retire a minted Catalog Client (egret-warehouse catalogs).
    if let Some(clientid) = client_row_id(&cat.config) {
        let _ = st.storage.revoke_client(clientid).await;
    }
    // Re-render + roll.
    let req = build_provision_req(&st, &engine).await?;
    if let Some(prov) = st.engine_provisioner.as_ref() {
        if let Err(msg) = prov.update(&req).await {
            return Err(CpError::BadRequest(format!("engine update failed: {msg}")));
        }
    }
    Ok(StatusCode::OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(name: &str) -> ResourceGroupReq {
        ResourceGroupReq {
            name: name.into(),
            ..Default::default()
        }
    }
    fn sel(group: &str) -> ResourceSelectorReq {
        ResourceSelectorReq {
            group: group.into(),
            ..Default::default()
        }
    }

    #[test]
    fn validate_resource_groups_accepts_matching_roots() {
        let rg = ResourceGroupsReq {
            root_groups: vec![root("global")],
            selectors: vec![sel("global"), sel("global.etl")],
        };
        assert!(validate_resource_groups(&rg).is_ok());
    }

    #[test]
    fn validate_resource_groups_rejects_dangling_selector() {
        let rg = ResourceGroupsReq {
            root_groups: vec![root("global")],
            selectors: vec![sel("nope")],
        };
        assert!(matches!(
            validate_resource_groups(&rg),
            Err(CpError::BadRequest(_))
        ));
    }

    #[test]
    fn monitor_password_is_derived_from_id() {
        assert_eq!(
            monitor_password(Uuid::nil()),
            "egret-monitor-00000000-0000-0000-0000-000000000000"
        );
    }

    #[test]
    fn inject_monitor_user_adds_egret_monitor_when_auth_on() {
        let id = Uuid::new_v4();
        // An engine WITH auth (one human password user).
        let mut auth = Some(AuthReq {
            password_users: vec![PasswordEntryReq {
                username: "bob".into(),
                bcrypt_hash: "$2y$hash".into(),
            }],
            jwt: None,
        });
        inject_monitor_user(&mut auth, id).unwrap();

        let users = &auth.unwrap().password_users;
        // Both the human user AND egret-monitor are present.
        assert!(users.iter().any(|u| u.username == "bob"));
        let mon = users
            .iter()
            .find(|u| u.username == "egret-monitor")
            .expect("egret-monitor injected");
        // The hash stored for Trino stays `$2y$`.
        assert!(mon.bcrypt_hash.starts_with("$2y$"));
        // `bcrypt::verify` wants the `$2b$` prefix; relabel back for the check.
        let hash = mon.bcrypt_hash.replacen("$2y$", "$2b$", 1);
        assert!(bcrypt::verify(monitor_password(id), &hash).unwrap());
    }

    #[test]
    fn inject_monitor_user_noop_when_no_auth() {
        // A no-auth engine must NOT gain an auth block (egret-monitor not forced on).
        let mut auth: Option<AuthReq> = None;
        inject_monitor_user(&mut auth, Uuid::new_v4()).unwrap();
        assert!(auth.is_none());
    }
}
