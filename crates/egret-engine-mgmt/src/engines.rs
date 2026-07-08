//! Control-plane management for query engines (Trino on Kubernetes).
//!
//! Provisioning a Trino auto-wires it to egret's embedded Iceberg catalog: the
//! route resolves the warehouse + storage endpoint, mints a fresh Catalog Client
//! for the warehouse, and records that as the engine's PRIMARY catalog row. A
//! cluster can then carry additional catalogs (Postgres, tpch/tpcds, generic).
//!
//! This seam owns all connector-kind logic: [`build_catalog_req`]
//! translates each stored [`EngineCatalog`] row into a flat `{name, properties,
//! env}` [`CatalogReq`], and [`build_provision_req`] assembles the full
//! [`EngineProvisionReq`]. The kube-free provisioning ops (provision/status/
//! update/delete/restart) go through the `EngineProvisioningPort`, implemented
//! in egret-bin over `egret-orchestrator`. The kube-shaped ops
//! (exec/pod-logs/coordinator-request) go straight through
//! `egret_orchestrator::KubeClient`, held on `EngineMgmtState::kube`: `exec`
//! returns a `kube::api::AttachedProcess` so [`bridge_exec`] can pump it onto a
//! browser WebSocket.

use std::collections::BTreeMap;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use egret_auth::principal::Auth;
use egret_auth::require_permission;
use egret_catalog_api::{GrantInput, NewClient};
use futures_util::sink::SinkExt;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

use crate::EngineHttpError as CpError;
use crate::EngineMgmtState as CpState;
use crate::{Engine, EngineCatalog};

/// Default Trino image (official `trinodb/trino` tag) when the body omits it.
const DEFAULT_IMAGE_VERSION: &str = "470";
/// Kubernetes namespace engines are provisioned into (fixed for now).
const DEFAULT_NAMESPACE: &str = "egret-engines";
/// Shared per-warehouse role that engine Catalog Clients are bound to.
const ENGINE_ROLE_NAME: &str = "engine";

// Canonical engine-spec DTOs now live in the dependency-free `egret-engine-mgmt-api`
// contract crate (shared with `egret-orchestrator`); aliased here to the old
// control-plane names so `parse_*`/`build_provision_req`/`inject_monitor_user`
// bodies below (struct-literal construction + signatures) keep compiling unchanged.
// `pub` because `lib.rs` re-exports these names from `engines::{..}`.
pub use egret_engine_mgmt_api::{
    Affinity as AffinityReq, Auth as AuthReq, Autoscaling as AutoscalingReq,
    CatalogSpec as CatalogReq, ContainerPort as ContainerPortReq, CronTrigger as CronTriggerReq,
    DeploymentStrategy as DeploymentStrategyReq, EngineSpec as EngineProvisionReq,
    EngineStatus as EngineLiveStatus, EnvFromSource as EnvFromSourceReq,
    FaultTolerant as FaultTolerantReq, GracefulShutdown as GracefulShutdownReq,
    HeapConfig as HeapReq, HpaConfig as HpaConfigReq, Ingress as IngressReq, Jwt as JwtReq,
    KedaConfig as KedaConfigReq, KedaTrigger as KedaTriggerReq, MiniContainer as MiniContainerReq,
    NodeSelectorRequirement as NodeSelectorRequirementReq, ObjectMount as ObjectMountReq,
    PasswordEntry as PasswordEntryReq, PodStatus as EnginePod, Probe as ProbeReq,
    Probes as ProbesReq, PrometheusTrigger as PrometheusTriggerReq,
    ResourceGroup as ResourceGroupReq, ResourceGroups as ResourceGroupsReq,
    ResourceSelector as ResourceSelectorReq, RoleConfig as RoleConfigReq,
    RoleResources as RoleResourcesReq, SecurityContext as SecurityContextReq,
    SessionRule as SessionRuleReq, Toleration as TolerationReq,
    TopologySpreadConstraint as TopologySpreadConstraintReq, TrinoConfig as TrinoConfigReq,
    VolumeMount as VolumeMountReq, WorkerRoleConfig as WorkerRoleConfigReq,
};

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
        .route(
            "/engines/{id}/catalogs/{cid}",
            get(get_catalog).put(update_catalog).delete(remove_catalog),
        )
        .route("/engines/{id}/catalogs/{cid}/test", post(test_catalog))
        .route("/engines/{id}/pods/{pod}/logs", get(pod_logs))
        .route("/engines/{id}/pods/{pod}/logs/follow", get(pod_logs_follow))
        .route("/engines/{id}/pods/{pod}/exec", get(pod_exec))
        .route("/engines/{id}/restart", post(restart_engine))
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
    /// Link the new engine to an EXISTING Ranger profile (authz_services id).
    /// When omitted, provisioning creates a fresh profile named after the engine.
    #[serde(default)]
    authz_service_id: Option<i64>,
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
    secret_ref: Option<Uuid>,
}
impl From<EngineCatalog> for EngineCatalogDto {
    fn from(c: EngineCatalog) -> Self {
        EngineCatalogDto {
            id: c.id,
            name: c.name,
            kind: c.kind,
            is_primary: c.is_primary,
            secret_ref: c.secret_ref,
        }
    }
}

/// `GET /engines/{id}/catalogs/{cid}` payload: full stored config + secretRef,
/// with system-minted secret material redacted (see [`redact_catalog_config`]).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EngineCatalogDetailDto {
    id: Uuid,
    name: String,
    kind: String,
    is_primary: bool,
    secret_ref: Option<Uuid>,
    config: serde_json::Value,
}

/// Strip system-minted secret material from a catalog config before returning it
/// to clients. egret-warehouse configs carry a client credential; nothing else
/// should ever be exposed.
fn redact_catalog_config(kind: &str, mut config: serde_json::Value) -> serde_json::Value {
    if kind == "egret-warehouse" {
        if let Some(obj) = config.as_object_mut() {
            obj.remove("credential");
            obj.remove("clientRowId");
        }
    }
    config
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

/// `PUT /engines/{id}/catalogs/{cid}` body: replaces the stored `config` +
/// `secret_ref` in full (not a partial patch).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateCatalogBody {
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
        _ => {
            // Connector-first (v2): the stored `kind` IS the Trino connector
            // name and `config = { properties, secretProperties }`. Plaintext
            // properties render verbatim; each `secretProperties` entry gets
            // its own `${ENV:CAT_<prefix>_<n>}` placeholder resolved below.
            // `catalog_props_and_secrets` also carries the back-compat shim
            // for v1 rows (old `postgresql` host/port, old `generic`
            // connectorName/secretProperty).
            let (connector, plain, secrets) = catalog_props_and_secrets(cat);
            props.insert("connector.name".into(), connector);
            for (k, v) in plain {
                props.insert(k, v);
            }
            for (n, (k, secret_id)) in secrets.into_iter().enumerate() {
                let var = format!("CAT_{env_prefix}_{n}");
                props.insert(k, format!("${{ENV:{var}}}"));
                let val = st
                    .secrets
                    .resolve_plaintext(None, secret_id)
                    .await
                    .map_err(|e| CpError::BadRequest(format!("resolve secret: {e}")))?;
                env.insert(var, val);
            }
        }
    }
    Ok(CatalogReq {
        name: cat.name.clone(),
        properties: props,
        env,
    })
}

/// Connector name + (plaintext props, secret-backed props) resolved from a
/// stored catalog's config; see [`catalog_props_and_secrets`].
type CatalogPropsAndSecrets = (String, Vec<(String, String)>, Vec<(String, Uuid)>);

/// Resolve a catalog's connector name + (plaintext props, secret-backed props)
/// from its stored config. Supports the v2 shape
/// `{ properties, secretProperties }` and back-compat for v1 rows
/// (old `postgresql` host/port and old `generic` connectorName/secretProperty).
fn catalog_props_and_secrets(cat: &EngineCatalog) -> CatalogPropsAndSecrets {
    let cfg = &cat.config;
    // ── legacy postgres (kind == "postgresql", config.host present, no v2 properties)
    if cat.kind == "postgresql" && cfg.get("properties").is_none() && cfg.get("host").is_some() {
        let mut plain = vec![
            (
                "connection-url".to_string(),
                format!(
                    "jdbc:postgresql://{}:{}/{}",
                    cfg["host"].as_str().unwrap_or_default(),
                    cfg["port"].as_i64().unwrap_or(5432),
                    cfg["database"].as_str().unwrap_or_default()
                ),
            ),
            (
                "connection-user".to_string(),
                cfg["user"].as_str().unwrap_or_default().to_string(),
            ),
        ];
        let mut secrets = vec![];
        if let Some(id) = cat.secret_ref {
            secrets.push(("connection-password".to_string(), id));
        } else {
            plain.push(("connection-password".to_string(), String::new()));
        }
        return ("postgresql".to_string(), plain, secrets);
    }
    // ── connector name: v2 uses kind; legacy generic used config.connectorName
    let connector = if cat.kind == "generic" {
        cfg["connectorName"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    } else {
        cat.kind.clone()
    };
    // ── secret-backed properties
    let mut secrets: Vec<(String, Uuid)> = vec![];
    // v2: config.secretProperties = { prop: "<uuid>" }
    if let Some(obj) = cfg.get("secretProperties").and_then(|v| v.as_object()) {
        for (k, v) in obj {
            if let Some(id) = v.as_str().and_then(|s| Uuid::parse_str(s).ok()) {
                secrets.push((k.clone(), id));
            }
        }
    }
    // legacy generic single-secret: config.secretProperty + cat.secret_ref,
    // defaulting the property name to "connection-password" (matching the
    // old generic arm's default) when the row predates `secretProperty`.
    if secrets.is_empty() && cat.kind == "generic" {
        if let Some(id) = cat.secret_ref {
            let prop = cfg
                .get("secretProperty")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("connection-password")
                .to_string();
            secrets.push((prop, id));
        }
    }
    let secret_keys: std::collections::HashSet<&str> =
        secrets.iter().map(|(k, _)| k.as_str()).collect();
    // ── plaintext properties (skip any key that is secret-backed)
    let mut plain: Vec<(String, String)> = vec![];
    if let Some(obj) = cfg.get("properties").and_then(|v| v.as_object()) {
        for (k, v) in obj {
            if secret_keys.contains(k.as_str()) {
                continue;
            }
            plain.push((k.clone(), v.as_str().unwrap_or_default().to_string()));
        }
    }
    (connector, plain, secrets)
}

/// Build the full provisioning request from the engine + its stored catalogs + config.
async fn build_provision_req(st: &CpState, engine: &Engine) -> Result<EngineProvisionReq, CpError> {
    let cats = st.engine_mgmt.list_engine_catalogs(engine.id).await?;
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
        .storage_registry
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
        node_selector: str_map(&v["nodeSelector"]),
        tolerations: parse_tolerations(&v["tolerations"]),
        topology_spread: parse_topology_spread(&v["topologySpread"]),
        affinity: parse_affinity(&v["affinity"]),
        termination_grace_seconds: v["terminationGraceSeconds"].as_u64().map(|n| n as u32),
        additional_exposed_ports: parse_container_ports(&v["additionalExposedPorts"]),
    }
}

/// Parse the `tolerations[]` wire array. Omitted/`None` fields stay `None`.
fn parse_tolerations(v: &serde_json::Value) -> Vec<TolerationReq> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|t| TolerationReq {
                    key: t["key"].as_str().map(Into::into),
                    operator: t["operator"].as_str().map(Into::into),
                    value: t["value"].as_str().map(Into::into),
                    effect: t["effect"].as_str().map(Into::into),
                    toleration_seconds: t["tolerationSeconds"].as_i64(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse the `topologySpread[]` wire array.
fn parse_topology_spread(v: &serde_json::Value) -> Vec<TopologySpreadConstraintReq> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|t| TopologySpreadConstraintReq {
                    max_skew: t["maxSkew"].as_u64().unwrap_or(0) as u32,
                    topology_key: t["topologyKey"].as_str().unwrap_or_default().to_string(),
                    when_unsatisfiable: t["whenUnsatisfiable"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    match_labels: str_map(&t["matchLabels"]),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse the `affinity{nodeMatch,podAntiAffinityTopologyKey,rawOverride}` wire
/// object; `None` when absent. `rawOverride` carries opaque JSON through.
fn parse_affinity(v: &serde_json::Value) -> Option<AffinityReq> {
    if !v.is_object() {
        return None;
    }
    let node_match = v["nodeMatch"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|r| NodeSelectorRequirementReq {
                    key: r["key"].as_str().unwrap_or_default().to_string(),
                    operator: r["operator"].as_str().unwrap_or_default().to_string(),
                    values: r["values"]
                        .as_array()
                        .map(|vs| {
                            vs.iter()
                                .filter_map(|x| x.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();
    Some(AffinityReq {
        node_match,
        pod_anti_affinity_topology_key: v["podAntiAffinityTopologyKey"].as_str().map(Into::into),
        raw_override: v.get("rawOverride").filter(|r| !r.is_null()).cloned(),
    })
}

/// Parse an `additionalExposedPorts[]` / port-list wire array.
fn parse_container_ports(v: &serde_json::Value) -> Vec<ContainerPortReq> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|p| ContainerPortReq {
                    name: p["name"].as_str().unwrap_or_default().to_string(),
                    container_port: p["containerPort"].as_u64().unwrap_or(0) as u16,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse a `{runAsUser,runAsGroup,fsGroup,runAsNonRoot,rawOverride}` security
/// context; `None` when absent. `rawOverride` carries opaque JSON through.
fn parse_security_context(v: &serde_json::Value) -> Option<SecurityContextReq> {
    if !v.is_object() {
        return None;
    }
    Some(SecurityContextReq {
        run_as_user: v["runAsUser"].as_i64(),
        run_as_group: v["runAsGroup"].as_i64(),
        fs_group: v["fsGroup"].as_i64(),
        run_as_non_root: v["runAsNonRoot"].as_bool(),
        raw_override: v.get("rawOverride").filter(|r| !r.is_null()).cloned(),
    })
}

/// Parse a `volumeMounts[]` wire array.
fn parse_volume_mounts(v: &serde_json::Value) -> Vec<VolumeMountReq> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|m| VolumeMountReq {
                    name: m["name"].as_str().unwrap_or_default().to_string(),
                    mount_path: m["mountPath"].as_str().unwrap_or_default().to_string(),
                    sub_path: m["subPath"].as_str().map(Into::into),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse an init/sidecar `containers[]` wire array. `rawOverride` (opaque JSON)
/// wins verbatim downstream in the orchestrator; here it just carries through.
fn parse_mini_containers(v: &serde_json::Value) -> Vec<MiniContainerReq> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|c| MiniContainerReq {
                    name: c["name"].as_str().unwrap_or_default().to_string(),
                    image: c["image"].as_str().unwrap_or_default().to_string(),
                    command: c["command"]
                        .as_array()
                        .map(|x| {
                            x.iter()
                                .filter_map(|s| s.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default(),
                    args: c["args"]
                        .as_array()
                        .map(|x| {
                            x.iter()
                                .filter_map(|s| s.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default(),
                    env: str_map(&c["env"]),
                    volume_mounts: parse_volume_mounts(&c["volumeMounts"]),
                    raw_override: c.get("rawOverride").filter(|r| !r.is_null()).cloned(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse an `envFrom[]` wire array of `{configMapRef,secretRef}` refs.
fn parse_env_from(v: &serde_json::Value) -> Vec<EnvFromSourceReq> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|e| EnvFromSourceReq {
                    config_map_ref: e["configMapRef"].as_str().map(Into::into),
                    secret_ref: e["secretRef"].as_str().map(Into::into),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse a `{name,mountPath,subPath,defaultMode}` object-mount wire array.
fn parse_object_mounts(v: &serde_json::Value) -> Vec<ObjectMountReq> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|m| ObjectMountReq {
                    name: m["name"].as_str().unwrap_or_default().to_string(),
                    mount_path: m["mountPath"].as_str().unwrap_or_default().to_string(),
                    sub_path: m["subPath"].as_str().map(Into::into),
                    default_mode: m["defaultMode"].as_i64().map(|n| n as i32),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse the `deploymentStrategy{kind,maxSurge,maxUnavailable}` wire object.
fn parse_deployment_strategy(v: &serde_json::Value) -> Option<DeploymentStrategyReq> {
    if !v.is_object() {
        return None;
    }
    Some(DeploymentStrategyReq {
        kind: v["kind"].as_str().unwrap_or_default().to_string(),
        max_surge: v["maxSurge"].as_str().map(Into::into),
        max_unavailable: v["maxUnavailable"].as_str().map(Into::into),
    })
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
        image: None,
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
        autoscaling: parse_autoscaling(&v["autoscaling"]),
        // ── pod & scheduling (cluster-wide) — k8s object names / raw JSON, no
        //    secret resolution, so parsed straight from the wire here. ────────
        image: v["image"].as_str().map(Into::into),
        image_pull_secrets: v["imagePullSecrets"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        image_pull_policy: v["imagePullPolicy"].as_str().map(Into::into),
        service_account: v["serviceAccount"].as_str().map(Into::into),
        pod_security_context: parse_security_context(&v["podSecurityContext"]),
        container_security_context: parse_security_context(&v["containerSecurityContext"]),
        init_containers: parse_mini_containers(&v["initContainers"]),
        sidecar_containers: parse_mini_containers(&v["sidecarContainers"]),
        env_from: parse_env_from(&v["envFrom"]),
        config_mounts: parse_object_mounts(&v["configMounts"]),
        secret_mounts: parse_object_mounts(&v["secretMounts"]),
        deployment_strategy: parse_deployment_strategy(&v["deploymentStrategy"]),
        deployment_annotations: str_map(&v["deploymentAnnotations"]),
        revision_history_limit: v["revisionHistoryLimit"].as_u64().map(|n| n as u32),
        progress_deadline_seconds: v["progressDeadlineSeconds"].as_u64().map(|n| n as u32),
        share_process_namespace: v["shareProcessNamespace"].as_bool(),
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

/// Parse the `autoscaling { mode, hpa{...}, keda{...} }` wire object; `None` when
/// absent. All raw fields carry opaque JSON through with no secret resolution.
fn parse_autoscaling(v: &serde_json::Value) -> Option<AutoscalingReq> {
    if !v.is_object() {
        return None;
    }
    Some(AutoscalingReq {
        mode: v["mode"].as_str().unwrap_or_default().to_string(),
        hpa: parse_hpa_config(&v["hpa"]),
        keda: parse_keda_config(&v["keda"]),
    })
}

/// Parse the `hpa { maxReplicas, targetCpuPercent, targetMemoryPercent,
/// behaviorRaw }` wire object; `None` when absent.
fn parse_hpa_config(v: &serde_json::Value) -> Option<HpaConfigReq> {
    if !v.is_object() {
        return None;
    }
    Some(HpaConfigReq {
        max_replicas: v["maxReplicas"].as_u64().unwrap_or(0) as u32,
        target_cpu_percent: v["targetCpuPercent"].as_u64().map(|n| n as u32),
        target_memory_percent: v["targetMemoryPercent"].as_u64().map(|n| n as u32),
        behavior_raw: v.get("behaviorRaw").filter(|r| !r.is_null()).cloned(),
    })
}

/// Parse the `keda { minReplicaCount, maxReplicaCount, pollingInterval,
/// cooldownPeriod, triggers[], fallbackRaw }` wire object; `None` when absent.
fn parse_keda_config(v: &serde_json::Value) -> Option<KedaConfigReq> {
    if !v.is_object() {
        return None;
    }
    Some(KedaConfigReq {
        min_replica_count: v["minReplicaCount"].as_u64().unwrap_or(0) as u32,
        max_replica_count: v["maxReplicaCount"].as_u64().unwrap_or(0) as u32,
        polling_interval: v["pollingInterval"].as_u64().map(|n| n as u32),
        cooldown_period: v["cooldownPeriod"].as_u64().map(|n| n as u32),
        triggers: parse_keda_triggers(&v["triggers"]),
        fallback_raw: v.get("fallbackRaw").filter(|r| !r.is_null()).cloned(),
    })
}

/// Parse the `triggers[]` wire array of `{kind, cron, prometheus, rawOverride}`.
fn parse_keda_triggers(v: &serde_json::Value) -> Vec<KedaTriggerReq> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|t| KedaTriggerReq {
                    kind: t["kind"].as_str().unwrap_or_default().to_string(),
                    cron: parse_cron_trigger(&t["cron"]),
                    prometheus: parse_prometheus_trigger(&t["prometheus"]),
                    raw_override: t.get("rawOverride").filter(|r| !r.is_null()).cloned(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse a `{start, end, desiredReplicas, timezone}` cron trigger; `None` when absent.
fn parse_cron_trigger(v: &serde_json::Value) -> Option<CronTriggerReq> {
    if !v.is_object() {
        return None;
    }
    Some(CronTriggerReq {
        start: v["start"].as_str().unwrap_or_default().to_string(),
        end: v["end"].as_str().unwrap_or_default().to_string(),
        desired_replicas: v["desiredReplicas"].as_u64().unwrap_or(0) as u32,
        timezone: v["timezone"].as_str().unwrap_or_default().to_string(),
    })
}

/// Parse a `{serverAddress, metricName, query, threshold}` prometheus trigger;
/// `None` when absent.
fn parse_prometheus_trigger(v: &serde_json::Value) -> Option<PrometheusTriggerReq> {
    if !v.is_object() {
        return None;
    }
    Some(PrometheusTriggerReq {
        server_address: v["serverAddress"].as_str().unwrap_or_default().to_string(),
        metric_name: v["metricName"].as_str().unwrap_or_default().to_string(),
        query: v["query"].as_str().unwrap_or_default().to_string(),
        threshold: v["threshold"].as_str().unwrap_or_default().to_string(),
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
    require_permission(&p, "engine:read")?;
    Ok(Json(
        st.engine_mgmt
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
        .catalog
        .list_roles(warehouse_id)
        .await?
        .into_iter()
        .find(|r| r.name == ENGINE_ROLE_NAME)
    {
        return Ok(r.id);
    }
    let role = st
        .catalog
        .create_role(
            warehouse_id,
            ENGINE_ROLE_NAME,
            "engine",
            Some("Auto-created role for query engines"),
        )
        .await?;
    st.catalog
        .add_grant(
            role.id,
            GrantInput {
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

/// Whether the client referenced by `clientRowId` was minted by us (and should
/// be revoked when the catalog is removed). Defaults to `true` — missing or
/// non-`false` means minted — so it's a no-op for existing rows and only an
/// explicit `"mintedClient": false` opts a client out of revoke-on-remove.
fn is_minted_client(config: &serde_json::Value) -> bool {
    config.get("mintedClient").and_then(|v| v.as_bool()) != Some(false)
}

async fn create_engine(
    State(st): State<CpState>,
    Auth(p): Auth,
    Json(b): Json<CreateEngineBody>,
) -> Result<(StatusCode, Json<EngineDto>), CpError> {
    require_permission(&p, "engine:create")?;

    // 1. Resolve the warehouse; it must exist and be provisioned (has a
    //    lakekeeper counterpart) — engines connect by warehouse NAME.
    let wh = st
        .catalog
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
        .storage_registry
        .get_storage_conn(wh.storage_conn_id)
        .await?
        .ok_or_else(|| CpError::BadRequest("unknown storage connection".into()))?;
    let s3_endpoint = sc.endpoint.unwrap_or_default();

    // 3. Auto-wire: mint a fresh Catalog Client for the warehouse.
    let role_id = ensure_engine_role(&st, wh.id).await?;
    let client = st
        .catalog
        .create_client(NewClient {
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
        .engine_mgmt
        .create_engine(&b.name, "trino", &namespace, wh.id, &spec)
        .await
    {
        Ok(e) => e,
        Err(e) => {
            let _ = st.catalog.revoke_client(client.id).await;
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
        .engine_mgmt
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
        let _ = st.engine_mgmt.delete_engine(engine.id).await;
        let _ = st.catalog.revoke_client(client.id).await;
        return Err(e.into());
    }

    // 6. Build the provisioning request from the stored engine + catalogs.
    let req = match build_provision_req(&st, &engine).await {
        Ok(r) => r,
        Err(e) => {
            let _ = st.engine_mgmt.delete_engine(engine.id).await; // CASCADE removes the catalog
            let _ = st.catalog.revoke_client(client.id).await;
            return Err(e);
        }
    };

    // 7. Provision on Kubernetes; roll back the engine row (+ cascade catalog) +
    //    client on any failure.
    let Some(prov) = st.engine_provisioner.as_ref() else {
        let _ = st.engine_mgmt.delete_engine(engine.id).await;
        let _ = st.catalog.revoke_client(client.id).await;
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };
    if let Err(msg) = prov.provision(&req).await {
        let _ = st.engine_mgmt.delete_engine(engine.id).await;
        let _ = st.catalog.revoke_client(client.id).await;
        return Err(CpError::BadRequest(format!(
            "engine provisioning failed: {msg}"
        )));
    }

    // 8. Auto-bind the engine to a Ranger policy profile: link the admin-chosen
    // existing service, or create a fresh one named after the engine. Runs ONLY
    // here on the success path — after provisioning succeeded and every rollback
    // path above is passed — so a failed provision never creates an orphan
    // profile and a same-name retry stays clean. Best-effort: a binding failure
    // must not fail provisioning (the engine still works; the admin can relink
    // later), so we log and continue.
    let profile_id: Option<i64> = match b.authz_service_id {
        Some(sid) => match st.authz.list_services().await {
            Ok(svcs) if svcs.iter().any(|s| s.id == sid) => Some(sid),
            Ok(_) => {
                tracing::warn!(sid, "authz_service_id not found; leaving engine unlinked");
                None
            }
            Err(e) => {
                tracing::warn!(error = %e, "list_services failed during auto-bind");
                None
            }
        },
        None => {
            if let Err(e) = st.authz.ensure_trino_def().await {
                tracing::warn!(error = %e, "ensure_trino_def failed during auto-bind");
            }
            match st
                .authz
                .create_service(&b.name, "trino", &serde_json::json!({}))
                .await
            {
                Ok(svc) => Some(svc.id),
                Err(e) => {
                    tracing::warn!(error = %e, "create_service failed during auto-bind");
                    None
                }
            }
        }
    };
    if let Some(pid) = profile_id {
        if let Err(e) = st
            .engine_mgmt
            .set_engine_authz_service(engine.id, Some(pid))
            .await
        {
            tracing::warn!(error = %e, "set_engine_authz_service failed during auto-bind");
        }
    }

    Ok((StatusCode::CREATED, Json(engine.into())))
}

async fn get_engine(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<EngineDetailDto>, CpError> {
    require_permission(&p, "engine:read")?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
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
    require_permission(&p, "engine:update")?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;

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
    st.engine_mgmt
        .update_engine(id, &spec, &engine.status)
        .await?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;

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
    require_permission(&p, "engine:delete")?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    // Collect any minted Catalog Clients referenced by the catalog rows before the
    // engine (and its cascading catalogs) are removed.
    let cats = st
        .engine_mgmt
        .list_engine_catalogs(engine.id)
        .await
        .unwrap_or_default();
    if let Some(prov) = st.engine_provisioner.as_ref() {
        prov.delete(&engine.namespace, &engine.name)
            .await
            .map_err(|e| CpError::BadRequest(format!("engine delete failed: {e}")))?;
    }
    st.engine_mgmt.delete_engine(id).await?; // CASCADE removes engine_catalogs
    for c in cats {
        if let Some(cid) = client_row_id(&c.config) {
            let _ = st.catalog.revoke_client(cid).await;
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /engines/{id}/restart` — roll the coordinator + worker pods without
/// changing config (like `kubectl rollout restart`). Unlike `update`, this
/// always forces a roll: `prov.restart` patches a fresh timestamp annotation
/// onto both Deployments' pod templates regardless of whether the rendered
/// config changed.
async fn restart_engine(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, CpError> {
    require_permission(&p, "engine:provision")?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    let Some(prov) = st.engine_provisioner.as_ref() else {
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };
    prov.restart(&engine.namespace, &engine.name)
        .await
        .map_err(|e| CpError::BadRequest(format!("engine restart failed: {e}")))?;
    Ok(StatusCode::OK)
}

// ── catalog sub-routes ───────────────────────────────────────────────────────────

async fn list_catalogs(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<EngineCatalogDto>>, CpError> {
    require_permission(&p, "engine:read")?;
    let _engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    Ok(Json(
        st.engine_mgmt
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
    require_permission(&p, "engine:create")?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    // The stored `kind` IS the Trino connector name (or `egret-warehouse`); the
    // control-plane no longer gates on a fixed enum — any non-blank kind is a
    // valid connector. The frontend's own catalog-kind list (a UX hint, not a
    // gate) lives separately in `frontend/src/routes/control-plane/config/catalogKinds.ts`.
    if b.kind.trim().is_empty() {
        return Err(CpError::BadRequest("catalog kind is required".into()));
    }

    // For an egret-warehouse catalog, resolve the warehouse + endpoint and mint a
    // fresh client (same as the primary); other kinds store the body as-is.
    //
    // `config.clientId` (optional): run the catalog under the ROLE/grants of an
    // EXISTING Catalog Client of this warehouse, instead of the shared
    // `engine_default` role. `CatalogClient` never exposes a retrievable secret
    // (`client_secret` is generated once in `create_client` and only its argon2
    // hash is persisted — see `CatalogClient`/`CreatedClient` in
    // `egret-catalog/src/service/embedded.rs`), so we can't just reuse the
    // referenced client's credential directly. Instead we look up the chosen
    // client to read its `catalog_role_id`, validate it belongs to `wh`, and
    // mint a FRESH client under that same role. The new client is therefore
    // still ours (auto-created, its credential lives only on this catalog row),
    // so `mintedClient` stays `true` and revoke-on-remove in `remove_catalog`
    // still applies — the feature only changes which ROLE the minted client
    // inherits, not who "owns" the minted client.
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
            .catalog
            .get_warehouse(wh_id)
            .await?
            .ok_or_else(|| CpError::BadRequest("unknown warehouse".into()))?;
        if wh.lakekeeper_warehouse_id.is_none() {
            return Err(CpError::BadRequest(
                "warehouse is not provisioned yet".into(),
            ));
        }
        let sc = st
            .storage_registry
            .get_storage_conn(wh.storage_conn_id)
            .await?
            .ok_or_else(|| CpError::BadRequest("unknown storage connection".into()))?;
        let s3_endpoint = sc.endpoint.unwrap_or_default();

        // Optional `config.clientId`: inherit that client's role instead of the
        // default engine role. Validated to belong to `wh` via `list_clients`.
        let client_id_param = match b.config.get("clientId") {
            Some(serde_json::Value::Null) | None => None,
            Some(v) => Some(
                v.as_str()
                    .and_then(|s| Uuid::parse_str(s).ok())
                    .ok_or_else(|| {
                        CpError::BadRequest("config.clientId must be a UUID string".into())
                    })?,
            ),
        };
        let role_id = match client_id_param {
            Some(cid) => {
                st.catalog
                    .list_clients(wh.id)
                    .await?
                    .into_iter()
                    .find(|c| c.id == cid)
                    .ok_or_else(|| {
                        CpError::BadRequest(
                            "config.clientId does not reference a client of this warehouse".into(),
                        )
                    })?
                    .catalog_role_id
            }
            None => ensure_engine_role(&st, wh.id).await?,
        };
        let client = st
            .catalog
            .create_client(NewClient {
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
            "mintedClient": true,
        });
        (cfg, None, Some(client.id))
    } else {
        (b.config.clone(), b.secret_ref, None)
    };

    let cat = match st
        .engine_mgmt
        .create_engine_catalog(engine.id, &b.name, &b.kind, &config, secret_ref, false)
        .await
    {
        Ok(c) => c,
        Err(e) => {
            if let Some(cid) = minted_client {
                let _ = st.catalog.revoke_client(cid).await;
            }
            return Err(e.into());
        }
    };

    // Re-render + roll. On failure, undo the row (+ client).
    let rollback = |st: CpState, cat_id: Uuid, minted: Option<Uuid>| async move {
        let _ = st.engine_mgmt.delete_engine_catalog(cat_id).await;
        if let Some(cid) = minted {
            let _ = st.catalog.revoke_client(cid).await;
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

async fn get_catalog(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, cid)): Path<(Uuid, Uuid)>,
) -> Result<Json<EngineCatalogDetailDto>, CpError> {
    require_permission(&p, "engine:read")?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    let cat = st
        .engine_mgmt
        .get_engine_catalog(cid)
        .await?
        .ok_or(CpError::NotFound)?;
    if cat.engine_id != engine.id {
        return Err(CpError::NotFound);
    }
    Ok(Json(EngineCatalogDetailDto {
        id: cat.id,
        name: cat.name.clone(),
        kind: cat.kind.clone(),
        is_primary: cat.is_primary,
        secret_ref: cat.secret_ref,
        config: redact_catalog_config(&cat.kind, cat.config.clone()),
    }))
}

/// `POST /engines/{id}/catalogs/{cid}/test` payload.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CatalogTestDto {
    ok: bool,
    message: String,
}

/// Bound on the `/v1/statement` submit-then-poll loop: a hung/misbehaving
/// coordinator must not block the request forever.
const CATALOG_TEST_MAX_POLLS: u32 = 50;

/// Quote a catalog name as a SQL identifier, doubling any embedded `"` — the
/// name is admin-controlled (not user free-text end to end), but this keeps
/// the statement well-formed regardless.
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Extract `nextUri`'s path (+ query) for [`crate::monitor::coordinator_call`],
/// which takes a path relative to the coordinator, not a full URL.
fn next_uri_path(next_uri: &str) -> String {
    match reqwest::Url::parse(next_uri) {
        Ok(u) => match u.query() {
            Some(q) => format!("{}?{q}", u.path()),
            None => u.path().to_string(),
        },
        Err(_) => next_uri.to_string(),
    }
}

/// Render a non-2xx coordinator response as a test-result message.
fn coordinator_error_message(code: u16, body: &[u8]) -> String {
    format!(
        "coordinator returned {code}: {}",
        String::from_utf8_lossy(body).trim()
    )
}

/// `POST /engines/{id}/catalogs/{cid}/test` — "test connection": runs
/// `SHOW SCHEMAS FROM "<catalog>"` against the engine's Trino coordinator via
/// the native-monitor coordinator proxy ([`crate::monitor::coordinator_call`],
/// same service name + auth headers as query monitoring) and reports whether
/// the query reaches `FINISHED`. Submits `POST /v1/statement`, then follows
/// `nextUri` (GET) until a terminal state, an `error` object, or
/// [`CATALOG_TEST_MAX_POLLS`] iterations are exhausted (→ `ok:false`,
/// "timed out"). Never returns a transport/coordinator-shape failure as an
/// HTTP error status — those map to `ok:false` with an explanatory message —
/// EXCEPT when the coordinator itself is unreachable or kubernetes isn't
/// configured, which surface as the usual `CpError` (mirrors the other
/// coordinator-proxied routes in this file and in `monitor.rs`).
async fn test_catalog(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, cid)): Path<(Uuid, Uuid)>,
) -> Result<Json<CatalogTestDto>, CpError> {
    require_permission(&p, "engine:read")?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    let cat = st
        .engine_mgmt
        .get_engine_catalog(cid)
        .await?
        .ok_or(CpError::NotFound)?;
    if cat.engine_id != engine.id {
        return Err(CpError::NotFound);
    }

    let sql = format!("SHOW SCHEMAS FROM {}", quote_ident(&cat.name));
    let (code, body) = crate::monitor::coordinator_call(
        &st,
        &engine,
        "POST",
        "/v1/statement",
        Some(sql.into_bytes()),
    )
    .await?;
    if !(200..300).contains(&code) {
        return Ok(Json(CatalogTestDto {
            ok: false,
            message: coordinator_error_message(code, &body),
        }));
    }
    let mut v: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            return Ok(Json(CatalogTestDto {
                ok: false,
                message: "coordinator returned a non-JSON response".into(),
            }))
        }
    };

    for _ in 0..=CATALOG_TEST_MAX_POLLS {
        if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
            return Ok(Json(CatalogTestDto {
                ok: false,
                message: err["message"]
                    .as_str()
                    .unwrap_or("query failed")
                    .to_string(),
            }));
        }
        let state = v["stats"]["state"].as_str().unwrap_or("");
        let Some(next_uri) = v["nextUri"].as_str() else {
            return Ok(Json(CatalogTestDto {
                ok: state == "FINISHED",
                message: if state == "FINISHED" {
                    "ok".into()
                } else {
                    format!("query ended in state {state}")
                },
            }));
        };
        let path = next_uri_path(next_uri);
        let (code, body) =
            crate::monitor::coordinator_call(&st, &engine, "GET", &path, None).await?;
        if !(200..300).contains(&code) {
            return Ok(Json(CatalogTestDto {
                ok: false,
                message: coordinator_error_message(code, &body),
            }));
        }
        v = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(_) => {
                return Ok(Json(CatalogTestDto {
                    ok: false,
                    message: "coordinator returned a non-JSON response".into(),
                }))
            }
        };
    }
    Ok(Json(CatalogTestDto {
        ok: false,
        message: "timed out".into(),
    }))
}

async fn remove_catalog(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, cid)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, CpError> {
    require_permission(&p, "engine:delete")?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    let cat = st
        .engine_mgmt
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
    st.engine_mgmt.delete_engine_catalog(cat.id).await?;
    // Best-effort: retire a minted Catalog Client (egret-warehouse catalogs).
    // Gated on `mintedClient` so a client we did NOT mint (config explicitly
    // sets `mintedClient: false`) is never revoked out from under its owner.
    // `add_catalog` always mints its own client today (even when inheriting an
    // existing client's role via `config.clientId`), so this is always `true`
    // in practice — the guard exists for forward-compat with a future
    // "adopt an existing client verbatim" path.
    if let Some(clientid) = client_row_id(&cat.config) {
        if is_minted_client(&cat.config) {
            let _ = st.catalog.revoke_client(clientid).await;
        }
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

async fn update_catalog(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    Json(b): Json<UpdateCatalogBody>,
) -> Result<Json<EngineCatalogDto>, CpError> {
    require_permission(&p, "engine:update")?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    let prev = st
        .engine_mgmt
        .get_engine_catalog(cid)
        .await?
        .ok_or(CpError::NotFound)?;
    // Guard against cross-engine updates via a mismatched path.
    if prev.engine_id != engine.id {
        return Err(CpError::NotFound);
    }
    if prev.is_primary {
        return Err(CpError::BadRequest(
            "cannot edit the primary catalog".into(),
        ));
    }
    if prev.kind == "egret-warehouse" {
        return Err(CpError::BadRequest(
            "egret-warehouse catalogs are managed and not editable".into(),
        ));
    }

    // Persist the new config, then re-provision. If re-provisioning fails, restore
    // the previous config/secret so the stored row never leads the running cluster.
    let updated = st
        .engine_mgmt
        .update_engine_catalog(cid, &b.config, b.secret_ref)
        .await?
        .ok_or(CpError::NotFound)?;

    let restore = |st: CpState, prev: EngineCatalog| async move {
        let _ = st
            .engine_mgmt
            .update_engine_catalog(prev.id, &prev.config, prev.secret_ref)
            .await;
    };

    let req = match build_provision_req(&st, &engine).await {
        Ok(r) => r,
        Err(e) => {
            restore(st.clone(), prev).await;
            return Err(e);
        }
    };
    let Some(prov) = st.engine_provisioner.as_ref() else {
        restore(st.clone(), prev).await;
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };
    if let Err(msg) = prov.update(&req).await {
        restore(st.clone(), prev).await;
        return Err(CpError::BadRequest(format!("engine update failed: {msg}")));
    }

    Ok(Json(updated.into()))
}

// ── pod logs ─────────────────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct PodLogsQuery {
    tail: Option<i64>,
    previous: Option<bool>,
    container: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PodLogsDto {
    logs: String,
}

/// `GET /engines/{id}/pods/{pod}/logs?tail=&previous=&container=` — bounded tail
/// of a pod's logs. The pod must belong to the engine (verified against the live
/// pod list from `EngineProvisioner::status`), so one platform admin can't read
/// an arbitrary pod's logs by guessing a name.
async fn pod_logs(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, pod)): Path<(Uuid, String)>,
    axum::extract::Query(q): axum::extract::Query<PodLogsQuery>,
) -> Result<Json<PodLogsDto>, CpError> {
    require_permission(&p, "engine:read")?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    let Some(prov) = st.engine_provisioner.as_ref() else {
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };
    // Ownership: the pod must be one of this engine's live pods.
    let status = prov
        .status(&engine.namespace, &engine.name)
        .await
        .map_err(CpError::BadRequest)?;
    if !status.pods.iter().any(|pp| pp.name == pod) {
        return Err(CpError::NotFound);
    }
    let kube = st
        .kube
        .as_ref()
        .ok_or_else(|| CpError::BadRequest("kubernetes not configured".into()))?;
    let tail = q.tail.unwrap_or(1000).clamp(1, 5000);
    let logs = kube
        .pod_logs(
            &engine.namespace,
            &pod,
            q.container.as_deref(),
            tail,
            q.previous.unwrap_or(false),
        )
        .await
        .map_err(|e| CpError::BadRequest(format!("read logs: {e}")))?;
    Ok(Json(PodLogsDto { logs }))
}

// ── pod logs, live-following (websocket) ────────────────────────────────────

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct PodLogsFollowQuery {
    tail: Option<i64>,
}

/// `GET /engines/{id}/pods/{pod}/logs/follow?tail=` — upgrades to a WebSocket
/// and streams the pod's logs as they arrive (`kubectl logs -f` equivalent),
/// a ONE-directional stream (pod -> browser), unlike [`pod_exec`]'s
/// bidirectional bridge. Same ownership guard as [`pod_logs`]/[`pod_exec`]:
/// the pod must be one of the engine's live pods, checked BEFORE the WS
/// upgrade so an unknown pod 404s instead of upgrading and failing inside
/// the socket.
async fn pod_logs_follow(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, pod)): Path<(Uuid, String)>,
    axum::extract::Query(q): axum::extract::Query<PodLogsFollowQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, CpError> {
    require_permission(&p, "engine:read")?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    let Some(prov) = st.engine_provisioner.clone() else {
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };
    // Ownership: the pod must be one of this engine's live pods (same guard
    // as pod_logs/pod_exec), so one platform admin can't follow an arbitrary
    // pod's logs by guessing a name.
    let status = prov
        .status(&engine.namespace, &engine.name)
        .await
        .map_err(CpError::BadRequest)?;
    if !status.pods.iter().any(|pp| pp.name == pod) {
        return Err(CpError::NotFound);
    }
    let Some(kube) = st.kube.clone() else {
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };
    let namespace = engine.namespace.clone();
    let engine_name = engine.name.clone();
    let user = p.email.clone();
    let tail = q.tail.unwrap_or(1000).clamp(1, 5000);
    tracing::info!(engine = %engine_name, pod = %pod, user = %user, "pod log follow session opening");
    Ok(ws.on_upgrade(move |socket| async move {
        bridge_log_follow(kube, &namespace, &pod, tail, socket).await;
        tracing::info!(engine = %engine_name, pod = %pod, user = %user, "pod log follow session closed");
    }))
}

/// Pump log byte chunks from `KubeClient::pod_logs_stream` onto the
/// browser `WebSocket` as `Binary` frames, as they arrive. This is a
/// ONE-directional stream (pod -> browser): the only thing read back off the
/// socket is a `Close`, purely to stop the pump when the client disconnects
/// — there is no stdin to forward, unlike [`bridge_exec`]. Exits on stream
/// end/error, WS close, or a WS send error.
async fn bridge_log_follow(
    kube: Arc<dyn egret_orchestrator::KubeClient>,
    namespace: &str,
    pod: &str,
    tail_lines: i64,
    mut socket: WebSocket,
) {
    let mut stream = match kube.pod_logs_stream(namespace, pod, tail_lines).await {
        Ok(s) => s,
        Err(e) => {
            let _ = socket
                .send(Message::Text(format!("log stream failed: {e}").into()))
                .await;
            let _ = socket.close().await;
            return;
        }
    };

    loop {
        tokio::select! {
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    Some(Ok(_)) => {} // ignore any other inbound frame (no stdin on this stream)
                }
            }
            chunk = stream.next() => {
                match chunk {
                    Some(Ok(bytes)) => {
                        if socket.send(Message::Binary(bytes)).await.is_err() {
                            break;
                        }
                    }
                    Some(Err(_)) | None => break, // stream error or EOF
                }
            }
        }
    }
    let _ = socket.close().await;
}

// ── pod exec (websocket) ─────────────────────────────────────────────────────

/// A control message sent by the browser terminal over the same WS channel as
/// stdin bytes. Any WS **text** message that parses as `{"resize":{"cols":N,
/// "rows":M}}` is a resize; every other text/binary message is raw stdin. Kept
/// as a top-level, explicit JSON shape (rather than e.g. sniffing for a magic
/// byte) so the frontend contract is simple and stable.
#[derive(Deserialize)]
struct ExecCtrlMsg {
    resize: Option<ExecResize>,
}

#[derive(Deserialize)]
struct ExecResize {
    cols: u16,
    rows: u16,
}

/// `GET /engines/{id}/pods/{pod}/exec` — upgrades to a WebSocket and bridges it
/// to a `/bin/sh` session inside the pod via `KubeClient::exec`. Same
/// ownership guard as [`pod_logs`]: the pod must be one of the engine's live
/// pods, checked BEFORE the WS upgrade so an unknown pod 404s instead of
/// upgrading and failing inside the socket.
async fn pod_exec(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, pod)): Path<(Uuid, String)>,
    ws: WebSocketUpgrade,
) -> Result<Response, CpError> {
    require_permission(&p, "engine:provision")?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    let Some(prov) = st.engine_provisioner.clone() else {
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };
    // Ownership: the pod must be one of this engine's live pods (same guard as
    // pod_logs), so one platform admin can't exec into an arbitrary pod by
    // guessing a name.
    let status = prov
        .status(&engine.namespace, &engine.name)
        .await
        .map_err(CpError::BadRequest)?;
    if !status.pods.iter().any(|pp| pp.name == pod) {
        return Err(CpError::NotFound);
    }
    let Some(kube) = st.kube.clone() else {
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };
    let namespace = engine.namespace.clone();
    let engine_name = engine.name.clone();
    let user = p.email.clone();
    tracing::info!(engine = %engine_name, pod = %pod, user = %user, "pod exec session opening");
    Ok(ws.on_upgrade(move |socket| async move {
        bridge_exec(kube, &namespace, &pod, socket).await;
        tracing::info!(engine = %engine_name, pod = %pod, user = %user, "pod exec session closed");
    }))
}

/// Pump bytes between the browser `WebSocket` and a `/bin/sh` session attached
/// via `KubeClient::exec`. Browser `Text`/`Binary` messages become
/// stdin, EXCEPT a `Text` message that parses as [`ExecCtrlMsg`] with a
/// `resize` field, which instead resizes the pty via
/// `AttachedProcess::terminal_size()`. Pod stdout bytes become WS `Binary`
/// frames. Exits on WS close, stdout EOF, or a stdin/stdout I/O error; any
/// `KubeClient::exec` failure (e.g. `FakeKube`'s
/// "exec not supported") is sent back as one WS text frame before closing.
async fn bridge_exec(
    kube: Arc<dyn egret_orchestrator::KubeClient>,
    namespace: &str,
    pod: &str,
    mut socket: WebSocket,
) {
    // Give an interactive `bash` (the trinodb/trino image ships it), like
    // `kubectl exec -it <pod> -- bash`, falling back to `sh` only when bash is
    // absent. The bash/sh existence probe redirects ITS OWN stderr (not the
    // exec'd shell's), so command stderr still flows to the TTY. `TERM` is set
    // so full-screen tools (less, vi, top) render correctly in xterm.js.
    let shell = [
        "/bin/sh".to_string(),
        "-c".to_string(),
        "export TERM=xterm-256color; if command -v bash >/dev/null 2>&1; then exec bash; else exec sh; fi".to_string(),
    ];
    let mut attached = match kube.exec(namespace, pod, &shell).await {
        Ok(a) => a,
        Err(e) => {
            let _ = socket
                .send(Message::Text(format!("exec failed: {e}").into()))
                .await;
            let _ = socket.close().await;
            return;
        }
    };
    let mut stdin = attached.stdin();
    let mut stdout = attached.stdout();
    let mut resize = attached.terminal_size();
    let mut buf = [0u8; 8192];

    loop {
        tokio::select! {
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Binary(data))) => {
                        let Some(w) = stdin.as_mut() else { break };
                        if w.write_all(&data).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        let resized = match serde_json::from_str::<ExecCtrlMsg>(&text) {
                            Ok(ExecCtrlMsg { resize: Some(sz) }) => {
                                if let Some(tx) = resize.as_mut() {
                                    let _ = tx
                                        .send(kube::api::TerminalSize {
                                            width: sz.cols,
                                            height: sz.rows,
                                        })
                                        .await;
                                }
                                true
                            }
                            _ => false,
                        };
                        if !resized {
                            let Some(w) = stdin.as_mut() else { break };
                            if w.write_all(text.as_bytes()).await.is_err() {
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
            n = async {
                match stdout.as_mut() {
                    Some(r) => r.read(&mut buf).await,
                    None => std::future::pending().await,
                }
            } => {
                match n {
                    Ok(0) | Err(_) => break, // stdout EOF or read error
                    Ok(n) => {
                        if socket.send(Message::Binary(buf[..n].to_vec().into())).await.is_err() {
                            break;
                        }
                    }
                }
            }
        }
    }
    let _ = socket.close().await;
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

    #[test]
    fn redact_strips_warehouse_credential() {
        let cfg = serde_json::json!({
            "warehouse": "wh1", "s3_endpoint": "http://minio:9000",
            "credential": "id:secret", "clientRowId": "abc"
        });
        let out = redact_catalog_config("egret-warehouse", cfg);
        assert!(out.get("credential").is_none());
        assert!(out.get("clientRowId").is_none());
        assert_eq!(out["warehouse"], "wh1");
        // Non-warehouse kinds pass through untouched.
        let pg = serde_json::json!({"host": "db", "user": "u"});
        assert_eq!(redact_catalog_config("postgresql", pg.clone()), pg);
    }

    // ── build_catalog_req: generic connector + secret sourcing ──────────────

    fn generic_catalog(config: serde_json::Value, secret_ref: Option<Uuid>) -> EngineCatalog {
        EngineCatalog {
            id: Uuid::new_v4(),
            engine_id: Uuid::new_v4(),
            name: "gen".into(),
            kind: "generic".into(),
            config,
            secret_ref,
            is_primary: false,
            created_at: chrono::Utc::now(),
        }
    }

    fn dummy_engine() -> Engine {
        Engine {
            id: Uuid::new_v4(),
            name: "e".into(),
            r#type: "trino".into(),
            namespace: "egret-engines".into(),
            warehouse_id: Uuid::new_v4(),
            spec: serde_json::json!({}),
            status: "pending".into(),
            authz_service_id: None,
            created_at: chrono::Utc::now(),
        }
    }

    /// Build a real `EngineMgmtState` against the local dev Postgres (mirrors
    /// the self-skip pattern used by `tests/routes_api.rs`); `None` when
    /// `DATABASE_URL` isn't set so this suite doesn't fail in environments
    /// without a database.
    async fn test_state() -> Option<CpState> {
        let dsn = std::env::var("DATABASE_URL").ok()?;
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        db.migrate().await.expect("migrate");
        let crypto = egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef")
            .expect("build test crypto");
        let secrets = egret_secrets::SecretStore::new(db.clone(), Some(crypto));
        let workspace_secrets = egret_secrets::WorkspaceSecretStore::new(db.clone(), secrets);
        Some(CpState {
            engine_mgmt: crate::EngineMgmtStore::new(db.clone()),
            catalog: std::sync::Arc::new(egret_catalog_engine::CatalogStore::new(db.clone())),
            storage_registry: egret_storage::StorageRegistry::new(db.clone()),
            authz: std::sync::Arc::new(egret_policy::PolicyStore::new(db.clone())),
            secrets: workspace_secrets,
            sessions: egret_auth::session::SessionService::new(
                db.clone(),
                chrono::Duration::hours(24),
                chrono::Duration::minutes(60),
            ),
            engine_provisioner: None,
            kube: None,
            engine_egret_url: "http://egret:8080".into(),
            engine_jwks_uri: "https://idp/jwks".into(),
        })
    }

    /// WITHOUT a `secret_ref` the generic arm must render exactly as before:
    /// verbatim properties, no env vars, no `${ENV:...}` placeholders.
    #[tokio::test]
    async fn generic_catalog_without_secret_ref_is_verbatim() {
        let Some(st) = test_state().await else {
            eprintln!(
                "skipping generic_catalog_without_secret_ref_is_verbatim: DATABASE_URL not set"
            );
            return;
        };
        let cfg = serde_json::json!({
            "connectorName": "mysql",
            "properties": {
                "connection-url": "jdbc:mysql://db:3306/x",
                "connection-user": "root",
            }
        });
        let cat = generic_catalog(cfg, None);
        let req = build_catalog_req(&st, &dummy_engine(), &cat, "T0")
            .await
            .expect("build_catalog_req");

        assert_eq!(req.properties["connector.name"], "mysql");
        assert_eq!(req.properties["connection-url"], "jdbc:mysql://db:3306/x");
        assert_eq!(req.properties["connection-user"], "root");
        assert!(req.env.is_empty());
        assert!(!req.properties.values().any(|v| v.contains("${ENV:")));
    }

    /// WITH a `secret_ref`, the designated property (default
    /// `connection-password`) is replaced by an `${ENV:...}` placeholder and
    /// the resolved plaintext lands in `env`, not in `properties`.
    #[tokio::test]
    async fn generic_catalog_with_secret_ref_injects_env_var() {
        let Some(st) = test_state().await else {
            eprintln!(
                "skipping generic_catalog_with_secret_ref_injects_env_var: DATABASE_URL not set"
            );
            return;
        };
        let secret_name = format!("t_{}", Uuid::new_v4().simple());
        let secret_id = st
            .secrets
            .create(None, &secret_name, None, "s3cr3t")
            .await
            .expect("create secret");
        let cfg = serde_json::json!({
            "connectorName": "mysql",
            "properties": {
                "connection-url": "jdbc:mysql://db:3306/x",
                "connection-password": "PLACEHOLDER",
            }
        });
        let cat = generic_catalog(cfg, Some(secret_id));
        let req = build_catalog_req(&st, &dummy_engine(), &cat, "T1")
            .await
            .expect("build_catalog_req");

        // Env var naming is now the uniform `CAT_<prefix>_<n>` scheme (was
        // `GEN_<prefix>`), shared by the v2 arm and this legacy shim path.
        assert_eq!(req.properties["connection-password"], "${ENV:CAT_T1_0}");
        assert_eq!(req.env["CAT_T1_0"], "s3cr3t");
        // The plaintext placeholder from `properties` must never leak through.
        assert!(!req.properties.values().any(|v| v == "PLACEHOLDER"));
    }

    // ── build_catalog_req: v2 connector-first shape + v1 back-compat shim ───

    /// Build a catalog row of an arbitrary `kind` (v2 rows use the connector
    /// name itself as `kind`, e.g. "mysql").
    fn catalog_of(
        kind: &str,
        config: serde_json::Value,
        secret_ref: Option<Uuid>,
    ) -> EngineCatalog {
        EngineCatalog {
            id: Uuid::new_v4(),
            engine_id: Uuid::new_v4(),
            name: "cat".into(),
            kind: kind.into(),
            config,
            secret_ref,
            is_primary: false,
            created_at: chrono::Utc::now(),
        }
    }

    /// (a) v2 connector row: `kind` IS the connector name, one secret-backed
    /// property resolves to `${ENV:CAT_<prefix>_0}` + a matching `env` entry;
    /// the plaintext password never appears in `properties`.
    #[tokio::test]
    async fn v2_connector_single_secret_property() {
        let Some(st) = test_state().await else {
            eprintln!("skipping v2_connector_single_secret_property: DATABASE_URL not set");
            return;
        };
        let secret_id = st
            .secrets
            .create(
                None,
                &format!("t_{}", Uuid::new_v4().simple()),
                None,
                "sekret",
            )
            .await
            .expect("create secret");
        let cfg = serde_json::json!({
            "properties": {"connection-url": "jdbc:mysql://h"},
            "secretProperties": {"connection-password": secret_id.to_string()},
        });
        let cat = catalog_of("mysql", cfg, None);
        let req = build_catalog_req(&st, &dummy_engine(), &cat, "P0")
            .await
            .expect("build_catalog_req");

        assert_eq!(req.properties["connector.name"], "mysql");
        assert_eq!(req.properties["connection-url"], "jdbc:mysql://h");
        assert_eq!(req.properties["connection-password"], "${ENV:CAT_P0_0}");
        assert_eq!(req.env["CAT_P0_0"], "sekret");
        assert!(!req.properties.values().any(|v| v == "sekret"));
    }

    /// (b) v2 connector row with TWO `secretProperties`: two distinct
    /// `${ENV:CAT_<prefix>_n}` placeholders and two matching `env` entries.
    /// Assert set membership since insertion order (a JSON object) is not
    /// guaranteed.
    #[tokio::test]
    async fn v2_connector_two_secret_properties() {
        let Some(st) = test_state().await else {
            eprintln!("skipping v2_connector_two_secret_properties: DATABASE_URL not set");
            return;
        };
        let secret_a = st
            .secrets
            .create(
                None,
                &format!("a_{}", Uuid::new_v4().simple()),
                None,
                "val-a",
            )
            .await
            .expect("create secret a");
        let secret_b = st
            .secrets
            .create(
                None,
                &format!("b_{}", Uuid::new_v4().simple()),
                None,
                "val-b",
            )
            .await
            .expect("create secret b");
        let cfg = serde_json::json!({
            "properties": {"connection-url": "jdbc:snowflake://h"},
            "secretProperties": {
                "connection-password": secret_a.to_string(),
                "connection-token": secret_b.to_string(),
            },
        });
        let cat = catalog_of("snowflake", cfg, None);
        let req = build_catalog_req(&st, &dummy_engine(), &cat, "P1")
            .await
            .expect("build_catalog_req");

        let vars: std::collections::HashSet<&str> = ["CAT_P1_0", "CAT_P1_1"].into_iter().collect();
        assert!(vars.contains(
            req.properties["connection-password"]
                .trim_start_matches("${ENV:")
                .trim_end_matches('}')
        ));
        assert!(vars.contains(
            req.properties["connection-token"]
                .trim_start_matches("${ENV:")
                .trim_end_matches('}')
        ));
        assert_ne!(
            req.properties["connection-password"],
            req.properties["connection-token"]
        );
        let resolved: std::collections::HashSet<&str> =
            req.env.values().map(|s| s.as_str()).collect();
        assert!(resolved.contains("val-a"));
        assert!(resolved.contains("val-b"));
        assert_eq!(req.env.len(), 2);
    }

    /// (c) v2 connector row with only `properties`, no `secretProperties`:
    /// verbatim passthrough, empty env (parity with the old no-secret path).
    #[tokio::test]
    async fn v2_connector_no_secrets_is_verbatim() {
        let Some(st) = test_state().await else {
            eprintln!("skipping v2_connector_no_secrets_is_verbatim: DATABASE_URL not set");
            return;
        };
        let cfg = serde_json::json!({
            "properties": {"connection-url": "jdbc:mysql://h", "connection-user": "root"},
        });
        let cat = catalog_of("mysql", cfg, None);
        let req = build_catalog_req(&st, &dummy_engine(), &cat, "P2")
            .await
            .expect("build_catalog_req");

        assert_eq!(req.properties["connector.name"], "mysql");
        assert_eq!(req.properties["connection-url"], "jdbc:mysql://h");
        assert_eq!(req.properties["connection-user"], "root");
        assert!(req.env.is_empty());
        assert!(!req.properties.values().any(|v| v.contains("${ENV:")));
    }

    /// (d) legacy `generic` row (kind="generic", config.connectorName +
    /// config.secretProperty + top-level secret_ref) still routes through the
    /// shim to the same connector.name + `${ENV:...}` shape.
    #[tokio::test]
    async fn legacy_generic_row_still_resolves() {
        let Some(st) = test_state().await else {
            eprintln!("skipping legacy_generic_row_still_resolves: DATABASE_URL not set");
            return;
        };
        let secret_id = st
            .secrets
            .create(
                None,
                &format!("t_{}", Uuid::new_v4().simple()),
                None,
                "legacy-pw",
            )
            .await
            .expect("create secret");
        let cfg = serde_json::json!({
            "connectorName": "mysql",
            "properties": {
                "connection-url": "jdbc:mysql://db:3306/x",
                "connection-password": "PLACEHOLDER",
            },
            "secretProperty": "connection-password",
        });
        let cat = generic_catalog(cfg, Some(secret_id));
        let req = build_catalog_req(&st, &dummy_engine(), &cat, "P3")
            .await
            .expect("build_catalog_req");

        assert_eq!(req.properties["connector.name"], "mysql");
        assert_eq!(req.properties["connection-password"], "${ENV:CAT_P3_0}");
        assert_eq!(req.env["CAT_P3_0"], "legacy-pw");
        assert!(!req.properties.values().any(|v| v == "PLACEHOLDER"));
    }

    /// (e) legacy `postgresql` row (kind="postgresql", config.host/port/user,
    /// top-level secret_ref) still builds a `connection-url` and a
    /// secret-backed `connection-password`.
    #[tokio::test]
    async fn legacy_postgres_row_still_resolves() {
        let Some(st) = test_state().await else {
            eprintln!("skipping legacy_postgres_row_still_resolves: DATABASE_URL not set");
            return;
        };
        let secret_id = st
            .secrets
            .create(
                None,
                &format!("t_{}", Uuid::new_v4().simple()),
                None,
                "pgpw",
            )
            .await
            .expect("create secret");
        let cfg = serde_json::json!({
            "host": "db.internal",
            "port": 5432,
            "database": "mydb",
            "user": "svc",
        });
        let cat = catalog_of("postgresql", cfg, Some(secret_id));
        let req = build_catalog_req(&st, &dummy_engine(), &cat, "P4")
            .await
            .expect("build_catalog_req");

        assert_eq!(req.properties["connector.name"], "postgresql");
        assert_eq!(
            req.properties["connection-url"],
            "jdbc:postgresql://db.internal:5432/mydb"
        );
        assert_eq!(req.properties["connection-user"], "svc");
        assert_eq!(req.properties["connection-password"], "${ENV:CAT_P4_0}");
        assert_eq!(req.env["CAT_P4_0"], "pgpw");
    }
}
