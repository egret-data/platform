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
use egret_catalog_api::{GrantInput, NewClient};
use futures_util::sink::SinkExt;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use utoipa::ToSchema;
use uuid::Uuid;

use egret_core::http::ErrorBody;

use crate::EngineHttpError as CpError;
use crate::EngineMgmtState as CpState;
use crate::{Engine, EngineCatalog};

/// Default Trino image (official `trinodb/trino` tag) when the body omits it.
const DEFAULT_IMAGE_VERSION: &str = "470";
/// Kubernetes namespace engines are provisioned into (fixed for now).
const DEFAULT_NAMESPACE: &str = "egret-engines";
/// Shared per-warehouse role that engine Catalog Clients are bound to.
const ENGINE_ROLE_NAME: &str = "engine";
/// Default StarRocks FE/CN images when the body omits them. Pinned to a 3.4.x
/// line because FE MySQL-protocol TLS (the delegation `tlsEnabled` path) is only
/// available in StarRocks open-source from v3.4.1 onwards — a 3.3 image can't
/// serve the SSL listener, so `tlsEnabled=true` would fail the handshake.
const DEFAULT_SR_FE_IMAGE: &str = "starrocks/fe-ubuntu:3.4-latest";
const DEFAULT_SR_CN_IMAGE: &str = "starrocks/cn-ubuntu:3.4-latest";
/// StarRocks FE MySQL-protocol (query) port — where the Iceberg external-catalog
/// auto-wire runs its `CREATE EXTERNAL CATALOG` DDL (`query_port` in `fe.conf`).
const STARROCKS_FE_MYSQL_PORT: u16 = 9030;

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
        // Engine↔workspace assignment (P1): list + set-full-assignment-set.
        .route(
            "/engines/{id}/workspaces",
            get(list_engine_workspaces).put(set_engine_workspaces),
        )
        // Native Trino monitor (query list / detail / kill / cluster).
        .merge(crate::monitor::routes())
}

/// Workspace-admin engine-assignment router (P1). Mounted under `/api/admin` by
/// `egret-server` (so the full paths are `/api/admin/workspaces/{id}/engines`),
/// separate from the CP engines router so it can carry the `/api/admin` prefix.
/// Shares [`CpState`] (which owns the [`crate::EngineAssignmentService`]).
pub fn workspace_engines_router() -> Router<CpState> {
    Router::new()
        .route(
            "/workspaces/{id}/engines",
            get(list_workspace_engines).post(assign_workspace_engine),
        )
        .route(
            "/workspaces/{id}/engines/{engine_id}",
            axum::routing::delete(unassign_workspace_engine),
        )
}

// ── DTOs (camelCase on the wire) ───────────────────────────────────────────────

#[derive(Deserialize, ToSchema)]
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
    #[schema(value_type = Object)]
    trino_config: Option<serde_json::Value>,
    /// Link the new engine to an EXISTING Ranger profile (authz_services id).
    /// When omitted, provisioning creates a fresh profile named after the engine.
    #[serde(default)]
    authz_service_id: Option<i64>,
    /// Which engine to provision: `"trino"` (default, back-compat) or
    /// `"starrocks"`. Selects between the Trino spec-building path and
    /// [`create_starrocks_engine`].
    #[serde(default = "default_engine_kind")]
    kind: String,
    /// StarRocks-specific config surface (FE/CN image + replicas + optional
    /// resources); only read when `kind == "starrocks"`. Storage (endpoint/
    /// path/keys) is NOT accepted here — it's derived from the warehouse's
    /// storage connection, same as the Trino S3 connector.
    #[serde(default)]
    starrocks: Option<StarRocksConfigBody>,
    /// Optional: a kubernetes.io/tls named secret to back the coordinator TLS.
    #[serde(default)]
    tls_cert_secret_ref: Option<Uuid>,
}

fn default_engine_kind() -> String {
    "trino".to_string()
}

/// Wire shape for `CreateEngineBody.starrocks`: FE/CN image + replicas +
/// optional per-role resources. Mirrors the `resources{cpuRequest,cpuLimit,
/// memRequest,memLimit}` shape [`parse_resources`] already understands.
#[derive(Deserialize, Clone, Default, ToSchema)]
#[serde(rename_all = "camelCase", default)]
struct StarRocksConfigBody {
    fe_image: Option<String>,
    cn_image: Option<String>,
    fe_replicas: Option<u32>,
    cn_replicas: Option<u32>,
    #[schema(value_type = Object)]
    fe_resources: Option<serde_json::Value>,
    #[schema(value_type = Object)]
    cn_resources: Option<serde_json::Value>,
    /// Opt into per-user delegation (Access parity slice 3): `"jwt"` |
    /// `"impersonate"` | omitted/`None` (no delegation — the slice-2 open
    /// engine default). The OIDC coordinates (jwks/issuer) and the
    /// impersonation service-user credential are NOT accepted here — they're
    /// derived from egret's own OIDC config / minted server-side (see
    /// [`create_starrocks_engine`]), same posture as Trino's
    /// `trinoConfig.auth.jwt` vs the delegator secret.
    delegation_mode: Option<String>,
    /// Enable FE SSL on the MySQL-protocol port (:9030) when delegation is
    /// configured. Defaults to `true` when `delegation_mode` is set; ignored
    /// otherwise. `false` opts out of TLS while still enabling delegation
    /// (e.g. a dev cluster where the extra listener setup isn't wanted).
    tls_enabled: Option<bool>,
    /// FE (`fe.conf`) override keys → merged over egret's managed base by the
    /// orchestrator. egret-managed keys ([`egret_engine_mgmt_api::SR_PROTECTED_FE_KEYS`])
    /// are rejected up front by [`validate_sr_fe_config`]. Omitted ⇒ preserved on update.
    fe_config: Option<std::collections::BTreeMap<String, String>>,
    /// CN (`cn.conf`) override keys → rendered into a ConfigMap by the orchestrator.
    /// Omitted ⇒ preserved on update.
    cn_config: Option<std::collections::BTreeMap<String, String>>,
    /// CN horizontal autoscaling `{ minReplicas?, maxReplicas, targetCpuPercent? }`.
    /// Stored/read as opaque JSON here; typed into `CnAutoscaling` at provision-req
    /// assembly. Omitted ⇒ preserved on update.
    #[schema(value_type = Object)]
    cn_autoscaling: Option<serde_json::Value>,
    /// Pod- & container-level scheduling / customization surface (nodeSelector,
    /// tolerations, affinity, sidecars, …). Opaque JSON here; typed into
    /// `StarRocksPod` at provision-req assembly. Omitted ⇒ preserved on update.
    #[schema(value_type = Object)]
    pod: Option<serde_json::Value>,
}

/// The provisioning parameters surfaced back to clients (subset of the stored
/// `spec` jsonb; internal bookkeeping keys are ignored on deserialize).
#[derive(Serialize, Deserialize, Default, ToSchema)]
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
    #[schema(value_type = Object)]
    trino_config: Option<serde_json::Value>,
    /// StarRocks equivalent of `trino_config`: the persisted `starrocksConfig`
    /// block (images/replicas/resources/delegation/feConfig/cnConfig/
    /// cnAutoscaling/pod) surfaced back so the UI can prefill the StarRocks
    /// config editor + show KPIs/auth on reload. Absent for Trino engines.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Object)]
    starrocks_config: Option<serde_json::Value>,
}

#[derive(Serialize, ToSchema)]
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

#[derive(Serialize, ToSchema)]
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
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct EngineCatalogDetailDto {
    id: Uuid,
    name: String,
    kind: String,
    is_primary: bool,
    secret_ref: Option<Uuid>,
    #[schema(value_type = Object)]
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

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct AddCatalogBody {
    name: String,
    kind: String,
    #[serde(default)]
    #[schema(value_type = Object)]
    config: serde_json::Value,
    #[serde(default)]
    secret_ref: Option<Uuid>,
}

/// `PUT /engines/{id}/catalogs/{cid}` body: replaces the stored `config` +
/// `secret_ref` in full (not a partial patch).
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct UpdateCatalogBody {
    #[serde(default)]
    #[schema(value_type = Object)]
    config: serde_json::Value,
    #[serde(default)]
    secret_ref: Option<Uuid>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct EnginePodDto {
    name: String,
    phase: String,
    ready: bool,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct EngineStatusDto {
    phase: String,
    message: Option<String>,
    pods: Vec<EnginePodDto>,
}

/// `GET /engines/{id}` payload: the engine row flattened with live status.
#[derive(Serialize, ToSchema)]
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
    // StarRocks engines assemble an entirely different `EngineSpec` (FE/CN roles
    // + builtin storage volume, no Trino catalogs/auth). Dispatch to the shared
    // StarRocks assembler so ANY generic caller of `build_provision_req` is
    // correct for a StarRocks engine. (`update_starrocks_engine` calls
    // `build_starrocks_provision_req` directly instead, so it can also capture
    // the delegation bootstrap a later task will consume.)
    if engine.r#type == "starrocks" {
        let wh = st
            .catalog
            .get_warehouse(engine.warehouse_id)
            .await?
            .ok_or_else(|| CpError::BadRequest("unknown warehouse".into()))?;
        let (req, _bootstrap) = build_starrocks_provision_req(st, engine, &wh).await?;
        return Ok(req);
    }
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
    // Skipped for jwt-delegation engines: their coordinator is JWT-only (no
    // PASSWORD authenticator), so a basic-auth monitor user can't authenticate —
    // the monitor degrades on jwt engines (acceptable).
    if !engine.is_jwt_delegation() {
        inject_monitor_user(&mut config.auth, engine.id)?;
    }
    // TLS for ANY delegation-capable engine (both modes): generate/reuse a
    // self-signed coordinator cert + mount the keystore so the coordinator serves
    // https and the connector can pin the cert. impersonate: encrypts the wire so
    // the delegator password is no longer sent in cleartext. jwt: binds the
    // caller's identity over TLS.
    if engine.supports_delegation() {
        let (keystore_pem, _cert) = ensure_tls_cert(st, engine).await?;
        config.tls = Some(egret_engine_mgmt_api::Tls {
            keystore_pem,
            port: 8443,
        });
    }
    // Ingress TLS from a typed secret: resolve the referenced kubernetes.io/tls
    // secret, project it into `{name}-ingress-tls`, and point the Ingress at it.
    // Typed ref wins over a raw `tls_secret` string when both are set. Shared by
    // provision and update (both build the spec here), so it stays consistent.
    if let Some(ing) = config.ingress.as_mut() {
        if let Some(ref_id) = ing.tls_cert_secret_ref {
            let (cert_pem, key_pem) = resolve_tls_cert_secret(st, ref_id).await?;
            ing.tls_cert_pem = Some(cert_pem);
            ing.tls_key_pem = Some(key_pem);
            ing.tls_secret = Some(format!("{}-ingress-tls", engine.name));
        }
    }
    // Impersonate-mode delegation: when auth is on, provision the `egret_delegator`
    // service user (generate + store its password once, inject its bcrypt into the
    // auth block so it lands in `password.db`) and seed the impersonate + query-
    // timeout Ranger policies onto the bound Trino service. Guarded on auth being
    // enabled — without a `password.db` the delegator can't basic-auth, so
    // impersonate delegation is meaningless (and `inject_delegator_user` no-ops).
    let ranger_service_name = engine.authz_service_id.map(|_| engine.name.clone());
    if config.auth.is_some() && engine.is_impersonate_delegation() {
        let (_secret_id, pw) = ensure_delegator_secret(st, engine).await?;
        inject_delegator_user(&mut config.auth, &pw)?;
        if let Some(service_name) = ranger_service_name.as_deref() {
            seed_delegator_policies(st, service_name).await?;
        }
    }
    // Trino requires a coordinator<->worker shared secret whenever auth is enabled.
    // Derive a stable per-engine value (both roles read it from the env Secret).
    if config.auth.is_some() {
        config.env.insert(
            "TRINO_INTERNAL_SECRET".into(),
            format!("egret-internal-{}", engine.id),
        );
    }
    // When the engine is bound to a Ranger profile (its trino profile is named
    // after the engine), template the apache-ranger access-control config so the
    // bundled plugin polls egret's Ranger-Admin download API (Access P3.2).
    let ranger =
        ranger_service_name
            .clone()
            .map(|service_name| egret_engine_mgmt_api::RangerBinding {
                service_name,
                egret_url: st.engine_egret_url.clone(),
                poll_interval_ms: 30_000,
            });
    Ok(EngineProvisionReq {
        name: engine.name.clone(),
        namespace: engine.namespace.clone(),
        image_version: engine.spec["imageVersion"]
            .as_str()
            .unwrap_or(DEFAULT_IMAGE_VERSION)
            .into(),
        catalogs,
        config,
        ranger,
        kind: egret_engine_mgmt_api::EngineKind::Trino,
        starrocks: None,
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
        // Resolved in build_provision_req: generated/reused per-engine keystore for
        // delegation-capable engines (jwt + impersonate).
        tls: None,
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
        tls_cert_secret_ref: v["tlsCertSecretRef"].as_str().and_then(|s| s.parse().ok()),
        tls_cert_pem: None,
        tls_key_pem: None,
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
        if j.is_object() {
            let jwks_uri = j["jwksUri"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(String::from)
                .unwrap_or_else(|| st.engine_jwks_uri.clone());
            // Enabling "accept egret JWT" with no JWKS to validate against would
            // template `http-server.authentication.jwt.key-file=<empty>` and crash
            // the Trino coordinator at boot (FileSigningKeyLocator /
            // io.jsonwebtoken.security.SecurityException). Reject at the API
            // boundary instead of provisioning a broken engine. Falls back to the
            // platform default JWKS above, so this fires only when BOTH the engine
            // block and the platform (OIDC) have no JWKS.
            if jwks_uri.is_empty() {
                return Err(CpError::BadRequest(
                    "Cannot enable engine JWT authentication: the platform has no JWKS \
                     (OIDC is not configured). Enable OIDC first, or disable JWT on this engine."
                        .into(),
                ));
            }
            Some(JwtReq {
                jwks_uri,
                required_audience: j["requiredAudience"].as_str().map(Into::into),
            })
        } else {
            None
        }
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

/// Inject the `egret_delegator` PASSWORD user (bcrypt of `password`) into an
/// engine's auth block so egret can basic-auth as it for impersonate-mode
/// delegated queries. No-op when auth is `None` (must not turn auth on). Uses the
/// same `$2b$`→`$2y$` relabel as the human users so Trino's jBCrypt file
/// authenticator accepts the hash.
fn inject_delegator_user(auth: &mut Option<AuthReq>, password: &str) -> Result<(), CpError> {
    let Some(auth) = auth.as_mut() else {
        return Ok(());
    };
    let bcrypt_hash = bcrypt::hash(password, bcrypt::DEFAULT_COST)
        .map_err(|e| CpError::BadRequest(format!("bcrypt delegator password: {e}")))?
        .replacen("$2b$", "$2y$", 1);
    auth.password_users.push(PasswordEntryReq {
        username: crate::delegator_policy::DELEGATOR_USERNAME.into(),
        bcrypt_hash,
    });
    Ok(())
}

/// Ensure the engine has a stored `egret_delegator` password secret; generate one
/// on first call and persist its named-secret id on the engine row (idempotent via
/// the `delegator_secret_id` column). The secret is a GLOBAL (workspace `None`)
/// named [`egret_secrets::WorkspaceSecretStore`] entry so `resolve_plaintext`
/// (which keys off the named id, like every other auth secret) can read it back at
/// materialize time. Returns `(secret_id, plaintext)`.
async fn ensure_delegator_secret(st: &CpState, engine: &Engine) -> Result<(Uuid, String), CpError> {
    if let Some(id) = engine.delegator_secret_id {
        let pw = st
            .secrets
            .resolve_plaintext(None, id)
            .await
            .map_err(|e| CpError::BadRequest(format!("resolve delegator secret: {e}")))?;
        return Ok((id, pw));
    }
    let pw: String = {
        use rand::RngCore;
        let mut b = [0u8; 24];
        rand::thread_rng().fill_bytes(&mut b);
        use base64::Engine as _;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
    };
    // Deterministic per-engine name so a partial prior run (secret created but the
    // engine column not yet set) recovers by name instead of erroring forever.
    let name = format!("egret-delegator-{}", engine.id);
    let id = match st
        .secrets
        .create(
            None,
            &name,
            Some("impersonate-mode Trino delegator password"),
            &pw,
        )
        .await
    {
        Ok(id) => id,
        Err(egret_secrets::SecretsError::AlreadyExists(_)) => {
            let meta = st
                .secrets
                .list(None)
                .await
                .map_err(|e| CpError::BadRequest(format!("list delegator secret: {e}")))?
                .into_iter()
                .find(|m| m.name == name)
                .ok_or_else(|| {
                    CpError::BadRequest("delegator secret vanished after conflict".into())
                })?;
            let pw = st
                .secrets
                .resolve_plaintext(None, meta.id)
                .await
                .map_err(|e| CpError::BadRequest(format!("resolve delegator secret: {e}")))?;
            st.engine_mgmt
                .set_delegator_secret(engine.id, meta.id)
                .await?;
            return Ok((meta.id, pw));
        }
        Err(e) => return Err(CpError::BadRequest(format!("store delegator secret: {e}"))),
    };
    st.engine_mgmt.set_delegator_secret(engine.id, id).await?;
    Ok((id, pw))
}

/// Ensure the engine has a stored StarRocks FE `root` password secret; generate one
/// on first call and persist its named-secret id on the engine row (idempotent via
/// the `root_secret_id` column). Mirrors [`ensure_delegator_secret`] exactly — same
/// RNG (24 random bytes, URL-safe base64) and same GLOBAL (workspace `None`) named
/// [`egret_secrets::WorkspaceSecretStore`] entry so `resolve_plaintext` (keyed off
/// the named id) can read it back at provision-req assembly time. Returns
/// `(secret_id, plaintext)`. Secure-by-default: `create_starrocks_engine` calls this
/// unconditionally, so every NEW StarRocks engine gets a strong root password; only
/// legacy engines (no `root_secret_id`) stay passwordless.
async fn ensure_root_password_secret(
    st: &CpState,
    engine: &Engine,
) -> Result<(Uuid, String), CpError> {
    if let Some(id) = engine.root_secret_id {
        let pw = st
            .secrets
            .resolve_plaintext(None, id)
            .await
            .map_err(|e| CpError::BadRequest(format!("resolve root password secret: {e}")))?;
        return Ok((id, pw));
    }
    let pw: String = {
        use rand::RngCore;
        let mut b = [0u8; 24];
        rand::thread_rng().fill_bytes(&mut b);
        use base64::Engine as _;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
    };
    // Deterministic per-engine name so a partial prior run (secret created but the
    // engine column not yet set) recovers by name instead of erroring forever.
    let name = format!("egret-root-{}", engine.id);
    let id = match st
        .secrets
        .create(None, &name, Some("StarRocks FE root password"), &pw)
        .await
    {
        Ok(id) => id,
        Err(egret_secrets::SecretsError::AlreadyExists(_)) => {
            let meta = st
                .secrets
                .list(None)
                .await
                .map_err(|e| CpError::BadRequest(format!("list root password secret: {e}")))?
                .into_iter()
                .find(|m| m.name == name)
                .ok_or_else(|| {
                    CpError::BadRequest("root password secret vanished after conflict".into())
                })?;
            let pw = st
                .secrets
                .resolve_plaintext(None, meta.id)
                .await
                .map_err(|e| CpError::BadRequest(format!("resolve root password secret: {e}")))?;
            st.engine_mgmt.set_root_secret(engine.id, meta.id).await?;
            return Ok((meta.id, pw));
        }
        Err(e) => {
            return Err(CpError::BadRequest(format!(
                "store root password secret: {e}"
            )))
        }
    };
    st.engine_mgmt.set_root_secret(engine.id, id).await?;
    Ok((id, pw))
}

/// Extract the leading `-----BEGIN CERTIFICATE-----`…`-----END CERTIFICATE-----`
/// block (including the trailing newline) from a concatenated cert+key keystore
/// PEM. Returns `None` when no certificate block is present.
fn extract_leading_cert(keystore_pem: &str) -> Option<String> {
    const END: &str = "-----END CERTIFICATE-----";
    let begin = keystore_pem.find("-----BEGIN CERTIFICATE-----")?;
    let end = keystore_pem[begin..].find(END)? + begin + END.len();
    let mut cert = keystore_pem[begin..end].to_string();
    cert.push('\n');
    Some(cert)
}

/// Ensure the engine has a TLS cert; generate a self-signed one (SAN = the
/// coordinator Service host + localhost) on first call, store the keystore
/// (cert+key PEM) in a GLOBAL egret secret + the public cert PEM on the engine
/// row, and reuse both on subsequent calls (idempotent via the `tls_secret_id` /
/// `coordinator_cert` columns). Returns `(keystore_pem, cert_pem)`.
/// Projection primitive: resolve a `kubernetes.io/tls` named secret to
/// `(cert_pem, key_pem)`. Reused wherever a typed TLS secret backs a k8s cert.
async fn resolve_tls_cert_secret(
    st: &CpState,
    secret_ref: Uuid,
) -> Result<(String, String), CpError> {
    let ts = st
        .secrets
        .resolve_typed(None, secret_ref)
        .await
        .map_err(|e| CpError::BadRequest(format!("resolve tls cert secret: {e}")))?;
    if ts.r#type != egret_secrets::secret_type::TLS {
        return Err(CpError::BadRequest(format!(
            "secret is not a {} secret",
            egret_secrets::secret_type::TLS
        )));
    }
    let get = |k: &str| {
        ts.data
            .get(k)
            .filter(|v| !v.is_empty())
            .cloned()
            .ok_or_else(|| CpError::BadRequest(format!("tls secret missing {k}")))
    };
    Ok((get("tls.crt")?, get("tls.key")?))
}

async fn ensure_tls_cert(st: &CpState, engine: &Engine) -> Result<(String, String), CpError> {
    if let (Some(sid), Some(cert)) = (engine.tls_secret_id, engine.coordinator_cert.clone()) {
        let keystore = st
            .secrets
            .resolve_plaintext(None, sid)
            .await
            .map_err(|e| CpError::BadRequest(format!("resolve engine tls secret: {e}")))?;
        return Ok((keystore, cert));
    }
    // A referenced kubernetes.io/tls secret backs the coordinator TLS; otherwise
    // fall back to a fresh self-signed cert (SAN = the coordinator host).
    let (cert_pem, key_pem) = if let Some(ref_id) = engine.tls_cert_secret_ref {
        resolve_tls_cert_secret(st, ref_id).await?
    } else {
        crate::tls_cert::generate_self_signed(&[engine.coordinator_host()])
            .map_err(|e| CpError::BadRequest(format!("generate engine tls cert: {e}")))?
    };
    let keystore = format!("{cert_pem}{key_pem}");
    // Deterministic per-engine name so a partial prior run (secret created but the
    // engine column not yet set) recovers by name instead of erroring forever.
    let name = format!("engine-tls-{}", engine.id);
    let sid = match st
        .secrets
        .create(
            None,
            &name,
            Some("jwt-mode coordinator keystore"),
            &keystore,
        )
        .await
    {
        Ok(id) => id,
        Err(egret_secrets::SecretsError::AlreadyExists(_)) => {
            let meta = st
                .secrets
                .list(None)
                .await
                .map_err(|e| CpError::BadRequest(format!("list engine tls secret: {e}")))?
                .into_iter()
                .find(|m| m.name == name)
                .ok_or_else(|| {
                    CpError::BadRequest("engine tls secret vanished after conflict".into())
                })?;
            let keystore = st
                .secrets
                .resolve_plaintext(None, meta.id)
                .await
                .map_err(|e| CpError::BadRequest(format!("resolve engine tls secret: {e}")))?;
            // The stored keystore (cert+key PEM from the prior run) is the source
            // of truth — our freshly generated `cert_pem` above is a different key
            // pair and would not match what the coordinator serves. Recover the
            // public cert from the stored keystore's leading certificate block.
            let cert = extract_leading_cert(&keystore).ok_or_else(|| {
                CpError::BadRequest("stored engine tls keystore has no certificate".into())
            })?;
            st.engine_mgmt.set_tls(engine.id, meta.id, &cert).await?;
            return Ok((keystore, cert));
        }
        Err(e) => return Err(CpError::BadRequest(format!("store engine tls secret: {e}"))),
    };
    st.engine_mgmt.set_tls(engine.id, sid, &cert_pem).await?;
    Ok((keystore, cert_pem))
}

/// Seed the two impersonate-mode Ranger policies (delegator-impersonate +
/// query-timeout) onto the engine's bound Trino service. No-op when the engine
/// has no bound Ranger service, or when a policy of the same name already exists
/// (idempotent across re-provisioning).
async fn seed_delegator_policies(st: &CpState, service_name: &str) -> Result<(), CpError> {
    use crate::delegator_policy as dp;
    let Some(svc) = st
        .authz
        .get_service_opt(service_name)
        .await
        .map_err(|e| CpError::BadRequest(format!("resolve ranger service: {e}")))?
    else {
        return Ok(());
    };
    for (name, body) in [
        (
            dp::IMPERSONATE_POLICY_NAME,
            dp::delegator_impersonate_policy(service_name),
        ),
        (
            dp::QUERY_TIMEOUT_POLICY_NAME,
            dp::query_timeout_policy(service_name),
        ),
    ] {
        if st
            .authz
            .policy_exists(svc.id, name)
            .await
            .map_err(|e| CpError::BadRequest(format!("check policy: {e}")))?
        {
            continue;
        }
        st.authz
            .create_policy(svc.id, name, &body)
            .await
            .map_err(|e| CpError::BadRequest(format!("seed policy: {e}")))?;
    }
    Ok(())
}

// ── handlers ───────────────────────────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/api/control-plane/engines",
    tag = "control-plane-engines",
    responses(
        (status = 200, description = "Provisioned query engines (Trino on Kubernetes)", body = Vec<EngineDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_engines(
    State(st): State<CpState>,
    Auth(p): Auth,
) -> Result<Json<Vec<EngineDto>>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:read").await?;
    // The stored `status` column is only ever the create-time value
    // ("provisioning") — nothing writes it back, so it would show every engine
    // as forever-provisioning on the list. Recompute the LIVE phase per engine
    // from the provisioner (same source `get_engine` uses), falling back to the
    // stored value if the live read fails or no provisioner is configured.
    let engines = st.engine_mgmt.list_engines().await?;
    let prov = st.engine_provisioner.as_ref();
    let mut out = Vec::with_capacity(engines.len());
    for engine in engines {
        let (kind, ns, name) = (
            engine.r#type.clone(),
            engine.namespace.clone(),
            engine.name.clone(),
        );
        let mut dto: EngineDto = engine.into();
        if let Some(prov) = prov {
            if let Ok(s) = prov.status(&kind, &ns, &name).await {
                dto.status = s.phase;
            }
        }
        out.push(dto);
    }
    Ok(Json(out))
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

#[utoipa::path(
    post,
    path = "/api/control-plane/engines",
    tag = "control-plane-engines",
    request_body = CreateEngineBody,
    responses(
        (status = 201, description = "Engine created and provisioned; primary catalog auto-wired to the warehouse", body = EngineDto),
        (status = 400, description = "Unknown/unprovisioned warehouse, invalid input, or provisioning failed", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:create permission", body = ErrorBody),
        (status = 409, description = "An engine with this name already exists", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn create_engine(
    State(st): State<CpState>,
    Auth(p): Auth,
    Json(b): Json<CreateEngineBody>,
) -> Result<(StatusCode, Json<EngineDto>), CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:create").await?;

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

    // Validate an optional coordinator-TLS secret reference up front (400 on a
    // missing / non-TLS / incomplete secret) so the row is only created once the
    // referenced cert resolves. `ensure_tls_cert` re-resolves it at provisioning.
    if let Some(ref_id) = b.tls_cert_secret_ref {
        resolve_tls_cert_secret(&st, ref_id).await?;
    }

    match egret_engine_mgmt_api::EngineKind::parse(&b.kind) {
        Some(egret_engine_mgmt_api::EngineKind::StarRocks) => {
            return create_starrocks_engine(st, b, wh).await;
        }
        Some(egret_engine_mgmt_api::EngineKind::Trino) => {}
        None => {
            return Err(CpError::BadRequest(format!(
                "unknown engine kind: {}",
                b.kind
            )));
        }
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
        .create_engine(
            &b.name,
            "trino",
            &namespace,
            wh.id,
            &spec,
            b.tls_cert_secret_ref,
        )
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
        // No admin-chosen service. Auto-create a fresh Ranger profile ONLY when
        // the platform Ranger integration is enabled. When it is off (the
        // default), leave the engine UNBOUND: the provisioner then writes no
        // `access-control.properties`, Trino defaults to allow-all, and per-user
        // delegated impersonation queries work (a Ranger-bound-but-unpolicied
        // Trino denies `egret_delegator` becoming the target user).
        None if !st.ranger_enabled => {
            tracing::info!(
                engine = %b.name,
                "ranger disabled; Trino engine provisioned without Ranger access-control (allow-all)"
            );
            None
        }
        None => {
            if let Err(e) = st.authz.ensure_trino_def().await {
                tracing::warn!(error = %e, "ensure_trino_def failed during auto-bind");
            }
            // Provision a TAG service alongside the trino profile (Access P3.2),
            // one tag service per trino profile, so tag-based masking/row-filter
            // policies can exist and `download_tags` has a service to serve. The
            // trino profile's config carries `tagService = <name>_tag`, the
            // standard Ranger field linking a resource service to its tag service.
            //
            // LIVE-ONLY: the `tagService` config key + any `ranger.plugin.tag.*`
            // download wiring are validated against the real Ranger tag plugin at
            // runtime, NOT in CI. If the plugin doesn't pick up tags, THIS is the
            // tuning spot. Best-effort like the rest of auto-bind: a failure here
            // never fails provisioning (the profile still works untagged).
            let tag_service_name = format!("{}_tag", b.name);
            let tag_wired = {
                if let Err(e) = st.authz.ensure_tag_def().await {
                    tracing::warn!(error = %e, "ensure_tag_def failed during auto-bind");
                }
                match st
                    .authz
                    .create_service(&tag_service_name, "tag", &serde_json::json!({}))
                    .await
                {
                    Ok(_) => true,
                    // A same-name tag service already exists (retry) — still wire it.
                    Err(egret_policy_api::PolicyError::AlreadyExists(_)) => true,
                    Err(e) => {
                        tracing::warn!(error = %e, "create tag service failed during auto-bind");
                        false
                    }
                }
            };
            let trino_config = if tag_wired {
                serde_json::json!({ "tagService": tag_service_name })
            } else {
                serde_json::json!({})
            };
            match st
                .authz
                .create_service(&b.name, "trino", &trino_config)
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
        } else {
            // Re-provision now that the engine is bound. The first `provision`
            // above ran BEFORE auto-bind set `authz_service_id`, so its spec had
            // no `RangerBinding` → the pod got no `access-control.properties`.
            // `build_provision_req` reads `authz_service_id`, so re-building here
            // and `update`-ing lands the Ranger plugin config (Access P3.2). Best
            // effort: a failure leaves a working-but-unenforced engine an admin
            // can re-provision. Refetch to pick up the freshly-set binding.
            match st.engine_mgmt.get_engine(engine.id).await {
                Ok(Some(bound)) => match build_provision_req(&st, &bound).await {
                    Ok(req2) => {
                        if let Err(msg) = prov.update(&req2).await {
                            tracing::warn!(engine = %bound.name, %msg,
                                "re-provision after auto-bind failed; engine unenforced until next update");
                        }
                    }
                    Err(e) => tracing::warn!(error = ?e,
                        "rebuild provision req after auto-bind failed"),
                },
                Ok(None) => {}
                Err(e) => tracing::warn!(error = %e, "refetch engine after auto-bind failed"),
            }
        }
    }

    Ok((StatusCode::CREATED, Json(engine.into())))
}

/// Assemble the StarRocks provisioning request from a STORED engine row (its
/// `spec.starrocksConfig`) + the warehouse's storage. Shared by create and
/// update so both feed the provisioner an identical `EngineSpec`. Resolves
/// storage creds, the optional Ranger binding, and (when the stored spec has a
/// delegation block) re-mints/reuses the TLS cert + delegator secret
/// idempotently, returning the `StarRocksDelegation` for the CR + the
/// `StarRocksDelegationBootstrap` the post-Ready wire step consumes.
///
/// Every read here comes from the persisted `spec.starrocksConfig` (NOT a create
/// body): `feImage`/`cnImage` (defaulting to [`DEFAULT_SR_FE_IMAGE`]/
/// [`DEFAULT_SR_CN_IMAGE`] when absent or blank), `feReplicas`/`cnReplicas`
/// (default 1), `feResources`/`cnResources` (parsed when a non-null value is
/// present), and the `delegation` `{mode, tlsEnabled}` block — matching what
/// `create_starrocks_engine` persisted so create and update produce an identical
/// [`EngineProvisionReq`]. The idempotent [`ensure_tls_cert`]/
/// [`ensure_delegator_secret`] calls reuse the engine's existing cert/secret
/// (keyed off `tls_secret_id`/`delegator_secret_id`) when present.
async fn build_starrocks_provision_req(
    st: &CpState,
    engine: &Engine,
    wh: &egret_catalog_api::Warehouse,
) -> Result<(EngineProvisionReq, Option<StarRocksDelegationBootstrap>), CpError> {
    // Storage backing for the FE/CN builtin storage volume — re-resolved from
    // the warehouse's storage connection so update (which has no create body)
    // feeds the provisioner the SAME S3 coords create did. Same resolution +
    // hard-fail-on-missing-secret posture as the Trino fault-tolerant path.
    let sc = st
        .storage_registry
        .get_storage_conn(wh.storage_conn_id)
        .await?
        .ok_or_else(|| CpError::BadRequest("unknown storage connection".into()))?;
    let storage_endpoint = sc.endpoint.clone().unwrap_or_default();
    let secret_ref = sc.secret_ref.ok_or_else(|| {
        CpError::BadRequest(
            "warehouse storage connection has no secret; cannot provision StarRocks".into(),
        )
    })?;
    let storage_secret_key = st
        .secrets
        .resolve_plaintext(None, secret_ref)
        .await
        .map_err(|e| CpError::BadRequest(format!("resolve storage secret: {e}")))?;
    // Namespace per engine so two StarRocks engines sharing a bucket don't
    // collide their shared-data storage roots (create used the same formula off
    // the engine name).
    let storage_path = format!("{}/starrocks/{}", sc.bucket, engine.name);

    // Desired FE/CN shape read back from the persisted spec (create wrote these).
    let cfg = engine.spec.get("starrocksConfig");
    let fe_image = cfg
        .and_then(|c| c.get("feImage"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_SR_FE_IMAGE.into());
    let cn_image = cfg
        .and_then(|c| c.get("cnImage"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_SR_CN_IMAGE.into());
    let fe_replicas = cfg
        .and_then(|c| c.get("feReplicas"))
        .and_then(|v| v.as_u64())
        .map(|n| n as u32)
        .unwrap_or(1);
    let cn_replicas = cfg
        .and_then(|c| c.get("cnReplicas"))
        .and_then(|v| v.as_u64())
        .map(|n| n as u32)
        .unwrap_or(1);
    // `feResources`/`cnResources` are persisted verbatim (a JSON object or
    // `null`); a null/absent value means "default sizing" (None), matching
    // create's `sr_body.fe_resources.as_ref().map(parse_resources)`.
    let fe_resources = cfg
        .and_then(|c| c.get("feResources"))
        .filter(|v| !v.is_null())
        .map(parse_resources);
    let cn_resources = cfg
        .and_then(|c| c.get("cnResources"))
        .filter(|v| !v.is_null())
        .map(parse_resources);

    // Deep-config surface read back into typed contract fields. Lenient like the
    // reads above: absent/null/parse-fail ⇒ empty map (feConfig/cnConfig) or
    // `None` (cnAutoscaling/pod), so a spec written before this surface existed
    // still assembles.
    let fe_config = cfg
        .and_then(|c| c.get("feConfig"))
        .filter(|v| !v.is_null())
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    let cn_config = cfg
        .and_then(|c| c.get("cnConfig"))
        .filter(|v| !v.is_null())
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    let cn_autoscaling = cfg
        .and_then(|c| c.get("cnAutoscaling"))
        .filter(|v| !v.is_null())
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok());
    let pod = cfg
        .and_then(|c| c.get("pod"))
        .filter(|v| !v.is_null())
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok());

    // Delegation opt-in read back from the stored `{mode, tlsEnabled}` block.
    // `tls_enabled` mirrors create: on only when delegation is configured,
    // defaulting to true (TLS-on) unless the stored block opted out.
    let delegation_cfg = cfg.and_then(|c| c.get("delegation"));
    let delegation_mode = delegation_cfg
        .and_then(|d| d.get("mode"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    let tls_enabled = delegation_mode.is_some()
        && delegation_cfg
            .and_then(|d| d.get("tlsEnabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

    // Ranger resolution — the SAME snippet `build_provision_req` uses for Trino:
    // bound profile (authz_service_id set) → RangerBinding named after the
    // engine; unbound → None.
    let ranger_service_name = engine.authz_service_id.map(|_| engine.name.clone());
    let ranger = ranger_service_name.map(|service_name| egret_engine_mgmt_api::RangerBinding {
        service_name,
        egret_url: st.engine_egret_url.clone(),
        poll_interval_ms: 30_000,
    });

    // Delegation wiring: provision (idempotently reuse) the TLS cert + delegator
    // secret the runtime `StarRocksDelegation` + post-Ready bootstrap need,
    // reusing the SAME generic helpers the Trino path uses (both key off just
    // `&Engine`). Minted for EITHER delegation mode (the bootstrap creates the
    // service user + GRANT IMPERSONATE alongside the JWT integration regardless
    // of mode).
    let mut delegation_cert_pem: Option<String> = None;
    let mut delegation_keystore_pem: Option<String> = None;
    if tls_enabled {
        let (keystore_pem, cert_pem) = ensure_tls_cert(st, engine).await?;
        delegation_cert_pem = Some(cert_pem);
        delegation_keystore_pem = Some(keystore_pem);
    }
    let mut delegation_service_password: Option<String> = None;
    if delegation_mode.is_some() {
        let (_secret_id, pw) = ensure_delegator_secret(st, engine).await?;
        delegation_service_password = Some(pw);
    }
    // OIDC coordinates are DERIVED from egret's own config (StarRocks has no
    // per-engine override surface). Soft-fails (warns, doesn't block) when OIDC
    // isn't configured — mirrors create.
    if delegation_mode.is_some()
        && (st.engine_jwks_uri.is_empty() || st.engine_oidc_issuer.is_empty())
    {
        tracing::warn!(
            engine = %engine.name,
            "starrocks delegation requested but OIDC is not configured on this deployment; \
             the JWT security integration will be templated with an empty issuer/jwks"
        );
    }
    let delegation = delegation_mode.map(|_| egret_engine_mgmt_api::StarRocksDelegation {
        jwks_url: st.engine_jwks_uri.clone(),
        issuer: st.engine_oidc_issuer.clone(),
        audience: "starrocks".into(),
        tls_cert_pem: delegation_cert_pem.clone().unwrap_or_default(),
        tls_keystore_pem: delegation_keystore_pem.clone().unwrap_or_default(),
        tls_enabled,
    });
    let delegation_bootstrap = delegation_mode.map(|_| StarRocksDelegationBootstrap {
        jwks_url: st.engine_jwks_uri.clone(),
        issuer: st.engine_oidc_issuer.clone(),
        audience: "starrocks".into(),
        tls_cert_pem: delegation_cert_pem.clone(),
        service_user: crate::delegator_policy::DELEGATOR_USERNAME.into(),
        service_password: delegation_service_password.clone().unwrap_or_default(),
    });

    // Secure-by-default root password: resolve the engine's stored FE `root`
    // password (minted at create) so the provisioner (Task 2) applies it via
    // initpwd. `Some` iff the engine carries a `root_secret_id` — legacy engines
    // provisioned before this column existed have `None` and stay passwordless.
    let root_password = match engine.root_secret_id {
        Some(sid) => Some(
            st.secrets
                .resolve_plaintext(None, sid)
                .await
                .map_err(|e| CpError::BadRequest(format!("resolve root password secret: {e}")))?,
        ),
        None => None,
    };

    let req = EngineProvisionReq {
        name: engine.name.clone(),
        namespace: engine.namespace.clone(),
        image_version: fe_image.clone(),
        config: TrinoConfigReq::default(),
        catalogs: vec![],
        ranger,
        kind: egret_engine_mgmt_api::EngineKind::StarRocks,
        starrocks: Some(egret_engine_mgmt_api::StarRocksConfig {
            fe_image,
            cn_image,
            fe_replicas,
            cn_replicas,
            storage_endpoint,
            storage_path,
            storage_access_key: sc.access_key.clone(),
            storage_secret_key,
            fe_resources,
            cn_resources,
            delegation,
            fe_config,
            cn_config,
            cn_autoscaling,
            pod,
            root_password,
        }),
    };
    Ok((req, delegation_bootstrap))
}

/// Validate a StarRocks body's `delegationMode` (the SAME rule create and update
/// both apply, up front, before any spec is persisted): an omitted/blank mode is
/// fine (no delegation), otherwise it MUST be `"jwt"` or `"impersonate"`.
fn validate_sr_delegation_mode(sr_body: &StarRocksConfigBody) -> Result<(), CpError> {
    if let Some(m) = sr_body.delegation_mode.as_deref().filter(|s| !s.is_empty()) {
        if m != "jwt" && m != "impersonate" {
            return Err(CpError::BadRequest(format!(
                "invalid starrocks delegationMode: {m} (expected \"jwt\" or \"impersonate\")"
            )));
        }
    }
    Ok(())
}

/// Reject a StarRocks body whose `feConfig` override tries to set a key egret
/// manages itself ([`egret_engine_mgmt_api::SR_PROTECTED_FE_KEYS`] — the
/// shared-data run mode, object-store coords, TLS keystore, and wire ports). The
/// SAME guard runs up front on both create and update, before any spec is
/// persisted, so a bad override 400s with nothing to roll back. The orchestrator
/// (Task 2, a separate crate) keeps its own belt-and-suspenders copy of the set
/// when it merges the config, but this is the primary API-level gate.
fn validate_sr_fe_config(sr_body: &StarRocksConfigBody) -> Result<(), CpError> {
    if let Some(fc) = sr_body.fe_config.as_ref() {
        for k in fc.keys() {
            if egret_engine_mgmt_api::SR_PROTECTED_FE_KEYS.contains(&k.as_str()) {
                return Err(CpError::BadRequest(format!(
                    "fe.conf key '{k}' is managed by egret and cannot be overridden"
                )));
            }
        }
    }
    Ok(())
}

/// Reject a StarRocks body whose `cnAutoscaling` block describes an autoscaler
/// that k8s / the operator would only fail on LATER (a deferred, hard-to-trace
/// error). Runs up front on both create and update, like the other SR guards.
///
/// `maxReplicas == 0` is the disabled sentinel (autoscaling OFF — the CR omits
/// `autoScalingPolicy` entirely, see the orchestrator), so validation is SKIPPED
/// for it; it's a valid "off". Only an ENABLED policy (`maxReplicas > 0`) is
/// checked:
/// - `targetCpuPercent` absent or `0`: in our CPU-only model an enabled policy
///   with no metric is a no-op autoscaler; k8s also rejects utilization `<= 0` at
///   HPA-create time. (`> 100` is VALID in k8s, so it's allowed.)
/// - `minReplicas == 0`: HPA requires `minReplicas >= 1`.
/// - `minReplicas > maxReplicas`: the operator/HPA rejects this.
fn validate_sr_autoscaling(sr_body: &StarRocksConfigBody) -> Result<(), CpError> {
    let Some(a) = sr_body.cn_autoscaling.as_ref() else {
        return Ok(());
    };
    let max = a.get("maxReplicas").and_then(|v| v.as_u64());
    // Disabled sentinel (or no maxReplicas at all) ⇒ nothing to validate.
    if max.unwrap_or(0) == 0 {
        return Ok(());
    }
    let max = max.unwrap();
    let cpu = a.get("targetCpuPercent").and_then(|v| v.as_u64());
    if cpu.unwrap_or(0) == 0 {
        return Err(CpError::BadRequest(
            "autoscaling requires a target CPU percent > 0".into(),
        ));
    }
    let min = a.get("minReplicas").and_then(|v| v.as_u64());
    if min == Some(0) {
        return Err(CpError::BadRequest(
            "autoscaling minReplicas must be >= 1".into(),
        ));
    }
    if let Some(m) = min {
        if m > max {
            return Err(CpError::BadRequest(
                "autoscaling minReplicas cannot exceed maxReplicas".into(),
            ));
        }
    }
    Ok(())
}

/// Build the stored engine `spec` JSON from a StarRocks body. On UPDATE,
/// `existing` is the current `engine.spec` so a PUT that omits a field preserves
/// it; on CREATE, pass `None` (every omitted field falls back to its create-time
/// default). No storage coords are ever written here — the FE/CN builtin storage
/// volume is re-resolved from the warehouse at provision-req assembly time, so
/// nothing storage-related leaks into the persisted spec.
///
/// Per-field rule: use the body value when present (non-blank for strings), else
/// the corresponding `existing.starrocksConfig.<field>`, else the create-time
/// default (`DEFAULT_SR_FE_IMAGE`/`DEFAULT_SR_CN_IMAGE`, replicas `1`, resources
/// `null`).
///
/// Delegation-preserve rule: the delegation block is ONLY rewritten when the
/// body carries a non-blank `delegationMode` (that's the Security tab editing
/// delegation). When `delegationMode` is absent/blank — the Config tab editing
/// image/replicas/resources — the EXISTING delegation block is preserved
/// verbatim, so an unrelated config edit never silently drops delegation. On
/// create (`existing` is `None`) an absent mode preserves "nothing" ⇒ `null`,
/// exactly matching create's prior behaviour.
fn build_starrocks_spec(
    sr_body: &StarRocksConfigBody,
    existing: Option<&serde_json::Value>,
) -> serde_json::Value {
    let existing_cfg = existing.and_then(|e| e.get("starrocksConfig"));
    let existing_str = |k: &str| {
        existing_cfg
            .and_then(|c| c.get(k))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .filter(|s| !s.trim().is_empty())
    };

    let fe_image = sr_body
        .fe_image
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| existing_str("feImage"))
        .unwrap_or_else(|| DEFAULT_SR_FE_IMAGE.into());
    let cn_image = sr_body
        .cn_image
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| existing_str("cnImage"))
        .unwrap_or_else(|| DEFAULT_SR_CN_IMAGE.into());
    let fe_replicas = sr_body
        .fe_replicas
        .or_else(|| {
            existing_cfg
                .and_then(|c| c.get("feReplicas"))
                .and_then(|v| v.as_u64())
                .map(|n| n as u32)
        })
        .unwrap_or(1);
    let cn_replicas = sr_body
        .cn_replicas
        .or_else(|| {
            existing_cfg
                .and_then(|c| c.get("cnReplicas"))
                .and_then(|v| v.as_u64())
                .map(|n| n as u32)
        })
        .unwrap_or(1);
    // Resources are stored verbatim (an object or `null`). A body value wins;
    // else preserve the existing value; else `null` (= default sizing).
    let fe_resources = sr_body
        .fe_resources
        .clone()
        .or_else(|| existing_cfg.and_then(|c| c.get("feResources")).cloned())
        .unwrap_or(serde_json::Value::Null);
    let cn_resources = sr_body
        .cn_resources
        .clone()
        .or_else(|| existing_cfg.and_then(|c| c.get("cnResources")).cloned())
        .unwrap_or(serde_json::Value::Null);

    // Delegation: rewrite ONLY when the body carries a mode; otherwise preserve
    // the existing block (see the delegation-preserve rule above). `tls_enabled`
    // mirrors create: on when a mode is set, defaulting to true unless opted out.
    let delegation = match sr_body.delegation_mode.as_deref().filter(|s| !s.is_empty()) {
        Some(m) => {
            let tls_enabled = sr_body.tls_enabled.unwrap_or(true);
            serde_json::json!({ "mode": m, "tlsEnabled": tls_enabled })
        }
        None => existing_cfg
            .and_then(|c| c.get("delegation"))
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    };

    // Deep-config surface (feConfig/cnConfig/cnAutoscaling/pod): same omit=preserve
    // rule as the fields above — a body value wins; else preserve the existing
    // stored value; else the empty/absent default (`{}` for the config maps, JSON
    // `null` for the optional autoscaling/pod blocks).
    let fe_config = sr_body
        .fe_config
        .clone()
        .map(|m| serde_json::to_value(m).unwrap_or_else(|_| serde_json::json!({})))
        .or_else(|| existing_cfg.and_then(|c| c.get("feConfig")).cloned())
        .unwrap_or_else(|| serde_json::json!({}));
    let cn_config = sr_body
        .cn_config
        .clone()
        .map(|m| serde_json::to_value(m).unwrap_or_else(|_| serde_json::json!({})))
        .or_else(|| existing_cfg.and_then(|c| c.get("cnConfig")).cloned())
        .unwrap_or_else(|| serde_json::json!({}));
    let cn_autoscaling = sr_body
        .cn_autoscaling
        .clone()
        .or_else(|| existing_cfg.and_then(|c| c.get("cnAutoscaling")).cloned())
        .unwrap_or(serde_json::Value::Null);
    let pod = sr_body
        .pod
        .clone()
        .or_else(|| existing_cfg.and_then(|c| c.get("pod")).cloned())
        .unwrap_or(serde_json::Value::Null);

    serde_json::json!({
        "imageVersion": fe_image,
        "starrocksConfig": {
            "feImage": fe_image,
            "cnImage": cn_image,
            "feReplicas": fe_replicas,
            "cnReplicas": cn_replicas,
            "feResources": fe_resources,
            "cnResources": cn_resources,
            "delegation": delegation,
            "feConfig": fe_config,
            "cnConfig": cn_config,
            "cnAutoscaling": cn_autoscaling,
            "pod": pod,
        },
    })
}

/// StarRocks create path (shared-data): builds a `StarRocksConfig` from the
/// body + the warehouse's storage connection (the FE/CN builtin storage-volume
/// object store — separate from the Iceberg EXTERNAL CATALOG auto-wire below),
/// persists the engine row with type `"starrocks"`, and provisions it.
///
/// Iceberg external-catalog auto-wire (T6): mirrors the Trino path's Catalog
/// Client minting EXACTLY (`ensure_engine_role` + `catalog.create_client`) and
/// records the same-shaped PRIMARY `egret-warehouse` catalog row, so the
/// client is visible/cleaned-up identically across engine kinds. Just like
/// Trino — whose bundled Iceberg-REST connector does the OAuth2
/// client-credentials exchange itself, driven by the `client_id:client_secret`
/// credential — StarRocks' external catalog is handed that SAME credential +
/// egret's token endpoint and self-refreshes its access token (no static
/// bearer to expire). A background step (spawned after `provision` succeeds,
/// see [`spawn_starrocks_catalog_wire`]) runs
/// [`starrocks_external_catalog_sql`] against the FE once it's Ready.
/// Any failure up to and including `provision` rolls back (revoke the client +
/// delete the engine row) exactly like the Trino path; the post-provision wire
/// step is best-effort and never fails the create (see its own docs).
///
/// Ranger: if the body links an EXISTING profile (`authzServiceId`) it's
/// validated and bound before the first provision, so the engine can come up
/// enforced; an omitted/invalid id just leaves it unbound (`ranger: None`).
/// Unlike the Trino path, this does NOT auto-create a fresh profile — that
/// needs its own StarRocks service-def wiring, which isn't plumbed through
/// `PolicyPort` yet.
async fn create_starrocks_engine(
    st: CpState,
    b: CreateEngineBody,
    wh: egret_catalog_api::Warehouse,
) -> Result<(StatusCode, Json<EngineDto>), CpError> {
    // Storage backing for the FE/CN builtin storage volume: same storage
    // connection the Trino S3 connector would use for this warehouse. The
    // stored `endpoint` is assumed pod-reachable already (that's how the Trino
    // path uses it verbatim for `s3.endpoint`, e.g. `http://host.docker.internal:9000`
    // in dev), so no host rewriting is needed here either.
    // Storage sanity checks BEFORE any engine row exists (nothing to roll back
    // yet): the connection must exist and — matching the Trino fault-tolerant
    // path — carry a secret, else a hard 400. The actual S3 coords + secret are
    // re-resolved from the warehouse inside `build_starrocks_provision_req` at
    // provision-req assembly time (below), so no bindings are threaded through.
    let sc = st
        .storage_registry
        .get_storage_conn(wh.storage_conn_id)
        .await?
        .ok_or_else(|| CpError::BadRequest("unknown storage connection".into()))?;
    if sc.secret_ref.is_none() {
        return Err(CpError::BadRequest(
            "warehouse storage connection has no secret; cannot provision StarRocks".into(),
        ));
    }

    let sr_body = b.starrocks.as_ref().cloned().unwrap_or_default();

    // Delegation opt-in (Access parity slice 3, S3-T2b): validate the mode up
    // front, before any engine row exists, so a bad value 400s with nothing to
    // roll back. `None`/omitted = no delegation (slice-2 default, open engine).
    // TLS-on defaulting when delegation is configured is applied inside
    // `build_starrocks_spec` (mirrors the Trino path, which provisions TLS for
    // BOTH delegation modes — a StarRocks jwt login sends the caller's JWT in
    // cleartext via the mysql_clear_password plugin, so TLS matters here too and
    // disabling it must be an explicit opt-out).
    validate_sr_delegation_mode(&sr_body)?;
    // Reject any `feConfig` override of an egret-managed fe.conf key up front.
    validate_sr_fe_config(&sr_body)?;
    // Reject an enabled autoscaling policy with out-of-range bounds up front, so
    // the admin gets a clean 400 instead of a deferred HPA-create failure.
    validate_sr_autoscaling(&sr_body)?;

    // ── Iceberg external-catalog auto-wire (T6), step 1: mint a Catalog Client
    // for the warehouse — the EXACT same call the Trino path uses
    // (`ensure_engine_role` + `catalog.create_client`). Minted BEFORE the
    // engine row exists, so a mint failure here needs no rollback (nothing to
    // roll back yet), matching the Trino path's ordering.
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

    let namespace = DEFAULT_NAMESPACE.to_string();
    // Stored spec (identical builder create and update share). The delegation
    // block is persisted so `store::Engine::delegation_mode`/
    // `supports_jwt_delegation`/`supports_delegation` (read back on every
    // subsequent load, including at `EngineAssignmentService::assign` time) see
    // the SAME mode this create call configured. An omitted mode serializes to a
    // JSON `null`, which `store::Engine::delegation_mode`'s `.get("mode")` chain
    // treats exactly like a wholly-absent key — it must NOT default to
    // impersonate the way Trino's `trinoConfig.delegationMode` does.
    let spec = build_starrocks_spec(&sr_body, None);

    let engine = match st
        .engine_mgmt
        .create_engine(
            &b.name,
            "starrocks",
            &namespace,
            wh.id,
            &spec,
            b.tls_cert_secret_ref,
        )
        .await
    {
        Ok(e) => e,
        Err(e) => {
            let _ = st.catalog.revoke_client(client.id).await;
            return Err(e.into());
        }
    };

    // Step 2: record the PRIMARY catalog row — same shape/kind Trino's
    // auto-wire uses (`egret-warehouse`), so `delete_engine`/list/detail/
    // `redact_catalog_config` treat a StarRocks engine's auto-wired catalog
    // identically to a Trino one. No `s3_endpoint` key: unlike Trino's
    // Iceberg-REST *connector config*, the StarRocks side never reads this
    // row back into a rendered config file (the CN/FE never see `cat_config`)
    // — it only carries the credential the background wire step consumes.
    let cat_config = serde_json::json!({
        "warehouse": wh.name,
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
        let _ = st.engine_mgmt.delete_engine(engine.id).await; // CASCADE removes the catalog
        let _ = st.catalog.revoke_client(client.id).await;
        return Err(e.into());
    }

    // Optional Ranger bind to an EXISTING profile (validated the same way the
    // Trino auto-bind step validates `authzServiceId`).
    if let Some(sid) = b.authz_service_id {
        match st.authz.list_services().await {
            Ok(svcs) if svcs.iter().any(|s| s.id == sid) => {
                if let Err(e) = st
                    .engine_mgmt
                    .set_engine_authz_service(engine.id, Some(sid))
                    .await
                {
                    tracing::warn!(error = %e, "set_engine_authz_service failed for starrocks create");
                }
            }
            Ok(_) => {
                tracing::warn!(
                    sid,
                    "authz_service_id not found; leaving starrocks engine unlinked"
                );
            }
            Err(e) => tracing::warn!(error = %e, "list_services failed during starrocks create"),
        }
    }
    // Secure-by-default (root-password task): mint + store a strong random FE
    // `root` password and persist its `root_secret_id` UNCONDITIONALLY — every NEW
    // StarRocks engine is passworded, closing the passwordless-root default. Rolls
    // back exactly like every other provisioning step above (delete the engine row,
    // revoke the catalog client). The persisted id is read back by the `get_engine`
    // refetch below so `build_starrocks_provision_req` resolves it into
    // `StarRocksConfig.root_password` (which Task 2's provisioner applies via
    // initpwd). Legacy engines (no `root_secret_id`) stay passwordless.
    if let Err(e) = ensure_root_password_secret(&st, &engine).await {
        let _ = st.engine_mgmt.delete_engine(engine.id).await;
        let _ = st.catalog.revoke_client(client.id).await;
        return Err(e);
    }

    let engine = match st.engine_mgmt.get_engine(engine.id).await {
        Ok(Some(e)) => e,
        Ok(None) => {
            let _ = st.engine_mgmt.delete_engine(engine.id).await;
            let _ = st.catalog.revoke_client(client.id).await;
            return Err(CpError::NotFound);
        }
        Err(e) => {
            let _ = st.engine_mgmt.delete_engine(engine.id).await;
            let _ = st.catalog.revoke_client(client.id).await;
            return Err(e.into());
        }
    };

    // Assemble the provisioning request from the STORED engine row — the SAME
    // shared helper a later update will call, so create and update feed the
    // provisioner an identical `EngineSpec`. Resolves storage creds + the
    // optional Ranger binding and (for a delegation-enabled spec) idempotently
    // mints/reuses the TLS cert + delegator secret. A failure here rolls back
    // exactly like every other provisioning step above (delete the engine row,
    // revoke the catalog client).
    let (req, delegation_bootstrap) = match build_starrocks_provision_req(&st, &engine, &wh).await {
        Ok(v) => v,
        Err(e) => {
            let _ = st.engine_mgmt.delete_engine(engine.id).await;
            let _ = st.catalog.revoke_client(client.id).await;
            return Err(e);
        }
    };

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

    // Step 3: best-effort, non-blocking post-provision wire. The FE takes
    // ~1-2 min to become Ready and (in dev) egret→FE reachability is a live
    // concern, so this is spawned rather than awaited — the create response
    // below returns as soon as `provision` above has kicked off, not after the
    // FE is actually queryable. A failure is logged, never surfaced to the
    // caller: the admin can retry by re-provisioning or (future) an explicit
    // "rewire catalog" action.
    // Present the minted FE `root` password (resolved into
    // `req.starrocks.root_password` by `build_starrocks_provision_req`) on the
    // post-Ready bootstrap connection; `None` for legacy engines keeps it
    // passwordless.
    let root_password = req.starrocks.as_ref().and_then(|s| s.root_password.clone());
    spawn_starrocks_catalog_wire(
        prov.clone(),
        engine.namespace.clone(),
        engine.name.clone(),
        st.engine_egret_url.clone(),
        wh.name.clone(),
        client.client_id.clone(),
        client.client_secret.clone(),
        root_password,
        delegation_bootstrap,
    );

    Ok((StatusCode::CREATED, Json(engine.into())))
}

/// Escape a value for embedding inside a double-quoted StarRocks property
/// string (`"…"`). Backslash and double-quote are the only two metacharacters
/// that can break out of a `"…"` value, so a `wh.name` (free-form, NOT
/// charset-validated anywhere) containing a `"` can't inject arbitrary DDL that
/// then runs as `root` on the FE. Backslash is escaped first so the escape
/// character it introduces isn't itself re-escaped.
fn sr_escape_dq(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Escape an identifier for a backtick-quoted StarRocks/MySQL identifier
/// (`` `…` ``). A literal backtick is escaped by doubling it — the same rule
/// MySQL/StarRocks use — so an identifier containing a backtick can't close the
/// quoting early and inject DDL.
fn sr_escape_ident(s: &str) -> String {
    s.replace('`', "``")
}

/// Build the StarRocks `CREATE EXTERNAL CATALOG` DDL that points a provisioned
/// StarRocks at egret's embedded Iceberg REST catalog, authenticating with
/// OAuth2 client-credentials — the EXACT same egret token endpoint +
/// `client_id:client_secret` credential the Trino path renders in
/// [`build_catalog_req`] (`iceberg.rest-catalog.security=OAUTH2` /
/// `…oauth2.server-uri` / `…oauth2.credential`). StarRocks' REST catalog client
/// self-refreshes its access token from these props, so — unlike a static
/// `iceberg.catalog.token` — the catalog does NOT die when a token expires.
///
/// StarRocks Iceberg-REST OAuth2 property keys — the Iceberg-STANDARD names
/// `iceberg.catalog.credential` (`client_id:client_secret`) +
/// `iceberg.catalog.oauth2-server-uri`. VERIFIED LIVE on StarRocks 3.3 (T8
/// smoke): the `iceberg.catalog.oauth2.credential`/`.server-uri` dot-form the
/// docs suggest (and StarRocks issue #57766 flags) is silently IGNORED — no
/// bearer is exchanged and catalog reads fail with "missing bearer token". The
/// standard names make StarRocks' Iceberg REST client self-exchange + refresh
/// the token, so — unlike a static `iceberg.catalog.token` — the catalog does
/// NOT die on expiry.
///
/// All interpolated values are escaped ([`sr_escape_dq`] / [`sr_escape_ident`])
/// so a warehouse/catalog name is never a DDL-injection vector (this DDL runs as
/// `root` on the FE).
fn starrocks_external_catalog_sql(
    catalog_name: &str,
    egret_url: &str,
    warehouse: &str,
    credential: &str,
) -> String {
    let name = sr_escape_ident(catalog_name);
    let uri = sr_escape_dq(&format!("{egret_url}/catalog"));
    let warehouse = sr_escape_dq(warehouse);
    let server_uri = sr_escape_dq(&format!("{egret_url}/oauth/token"));
    let credential = sr_escape_dq(credential);
    // OAuth2 uses the Iceberg-standard REST property NAMES (`credential` +
    // `oauth2-server-uri`), NOT the `iceberg.catalog.oauth2.*` dot-form —
    // verified live on StarRocks 3.3: the dot-form is silently ignored (no
    // bearer sent -> catalog reads fail with "missing bearer token"), while
    // these names make StarRocks' Iceberg REST client self-exchange + refresh
    // the token (mirrors the Trino path's OAuth2 config, no static token).
    format!(
        "CREATE EXTERNAL CATALOG IF NOT EXISTS `{name}` \
         PROPERTIES (\"type\"=\"iceberg\", \"iceberg.catalog.type\"=\"rest\", \
         \"iceberg.catalog.uri\"=\"{uri}\", \
         \"iceberg.catalog.warehouse\"=\"{warehouse}\", \
         \"iceberg.catalog.credential\"=\"{credential}\", \
         \"iceberg.catalog.oauth2-server-uri\"=\"{server_uri}\")"
    )
}

/// Build the `mysql_async` connection options for egret's own FE `root`
/// bootstrap connection. Factored out (and pure) so the password/user/TLS
/// wiring is unit-testable without a live FE — the wire path itself is
/// dev-DNS-blocked (egret-host -> in-cluster FE Service DNS), so this is the
/// only place the opts are asserted.
///
/// `root_password` is `Some(pw)` for any engine minted with a `root_secret_id`
/// (T1-T3): the FE `root` account is now passworded, so egret's post-Ready DDL
/// MUST present it or the connection is rejected. Legacy engines (no
/// `root_secret_id`) pass `None` and stay passwordless — the pre-hardening
/// behavior. The password is never logged and lives only inside the
/// `OptsBuilder` (which does not `Debug`-print it).
///
/// `tls_cert_pem` mirrors the connector's (S3-T1) TLS pinning — `Some(pem)`
/// pins the FE's egret-generated self-signed cert as the trust root (domain
/// validation skipped, same rationale as `StarRocksConnector::new`: the cert's
/// CN won't match the in-cluster Service DNS we dial) and runs the DDL over
/// TLS; `None` stays plain TCP.
fn starrocks_root_conn_opts(
    host: &str,
    port: u16,
    root_password: Option<&str>,
    tls_cert_pem: Option<&str>,
) -> mysql_async::OptsBuilder {
    let mut opts = mysql_async::OptsBuilder::default()
        .ip_or_hostname(host)
        .tcp_port(port)
        .user(Some("root"))
        // StarRocks' `mysql_service_nio_enabled` handshake rejects the
        // `prefer_socket` capability with "unknown variable 'socket'" —
        // matches `StarRocksConnector::new`'s own workaround.
        .prefer_socket(false);
    if let Some(pw) = root_password {
        opts = opts.pass(Some(pw));
    }
    if let Some(pem) = tls_cert_pem {
        let ssl = mysql_async::SslOpts::default()
            .with_root_certs(vec![mysql_async::PathOrBuf::from(pem.as_bytes().to_vec())])
            .with_danger_skip_domain_validation(true);
        opts = opts.ssl_opts(Some(ssl));
    }
    opts
}

/// Run one DDL statement against a StarRocks FE over the raw MySQL protocol,
/// as `root`. Direct `mysql_async` (not `egret_query`'s interactive
/// `StarRocksConnector`): this is a one-shot admin statement with no
/// caller-facing row cap/cancellation/result shape to thread through.
/// `root_password` presents the minted FE `root` password when the engine has
/// one (`None` -> passwordless, the legacy default); `tls_cert_pem` optionally
/// pins the FE cert and runs over TLS. See [`starrocks_root_conn_opts`] for the
/// connection-opts wiring.
async fn run_starrocks_ddl(
    host: &str,
    port: u16,
    sql: &str,
    root_password: Option<&str>,
    tls_cert_pem: Option<&str>,
) -> Result<(), String> {
    use mysql_async::prelude::Queryable;
    let opts = starrocks_root_conn_opts(host, port, root_password, tls_cert_pem);
    let mut conn = mysql_async::Conn::new(opts)
        .await
        .map_err(|e| e.to_string())?;
    conn.query_drop(sql).await.map_err(|e| e.to_string())?;
    let _ = conn.disconnect().await;
    Ok(())
}

/// Build the `CREATE SECURITY INTEGRATION` + `authentication_chain` DDL that
/// registers egret's OIDC/Keycloak JWKS with a StarRocks FE, mirroring
/// slice-1's manual `deploy/starrocks/init/security-integration.sql` (now
/// templated with the engine's own OIDC coordinates instead of a hand-edited
/// file). `principal_field=preferred_username` matches the Trino jwt-mode
/// config (`build_provision_req`'s
/// `http-server.authentication.jwt.principal-field`) and the connector's own
/// assumption (S3-T1's `StarRocksConnector::new` logs in as `session_user`,
/// which `submit_query` resolves from the same OIDC claim). `native,egret_oidc`
/// keeps the existing native/password chain working alongside JWT logins
/// (impersonate mode still needs the native chain for the service user).
///
/// All interpolated values are escaped ([`sr_escape_dq`]) — same DDL-injection
/// posture as [`starrocks_external_catalog_sql`]: this runs as `root` on the
/// FE, and `jwks_url`/`issuer`/`audience` are admin-controlled config, not
/// charset-validated anywhere upstream.
fn starrocks_jwt_integration_sql(jwks_url: &str, issuer: &str, audience: &str) -> Vec<String> {
    let jwks_url = sr_escape_dq(jwks_url);
    let issuer = sr_escape_dq(issuer);
    let audience = sr_escape_dq(audience);
    vec![
        format!(
            "CREATE SECURITY INTEGRATION egret_oidc PROPERTIES(\"type\"=\"authentication_jwt\", \
             \"jwks_url\"=\"{jwks_url}\", \"principal_field\"=\"preferred_username\", \
             \"required_issuer\"=\"{issuer}\", \"required_audience\"=\"{audience}\")"
        ),
        "ADMIN SET FRONTEND CONFIG (\"authentication_chain\"=\"native,egret_oidc\")".into(),
    ]
}

/// Build the `CREATE USER IF NOT EXISTS` DDL for the impersonation service
/// user (the connection's basic-auth login on the impersonate path — S3-T1's
/// `StarRocksConnector` logs in as this user, then runs `EXECUTE AS
/// <session_user>`). `IDENTIFIED BY` takes a single-quoted string literal
/// (StarRocks/MySQL `CREATE USER` grammar), so both the username and password
/// are single-quote-escaped — a username/password containing `'` must not be
/// able to close the literal early and inject DDL (this runs as `root`).
fn starrocks_create_service_user_sql(service_user: &str, password: &str) -> String {
    let esc_user = service_user.replace('\'', "''");
    let esc_pw = password.replace('\'', "''");
    format!("CREATE USER IF NOT EXISTS '{esc_user}'@'%' IDENTIFIED BY '{esc_pw}'")
}

/// Build the `GRANT IMPERSONATE` DDL authorizing the service user to run
/// `EXECUTE AS <any user> WITH NO REVERT` (the impersonate-mode primitive —
/// see `crate::execution::connector::starrocks::StarRocksConnector::switch_identity`
/// in `egret-query`). Backticks in the identifier are doubled (MySQL/StarRocks
/// identifier-escaping rule), matching [`sr_escape_ident`]'s convention.
fn starrocks_impersonation_grant_sql(service_user: &str) -> String {
    let esc = service_user.replace('`', "``");
    format!("GRANT IMPERSONATE ON ALL USERS TO `{esc}`")
}

/// Runtime bundle for the post-Ready delegation bootstrap (S3-T2 step 3,
/// wired up in S3-T2b): the JWT security integration + the impersonation
/// service user. `None` on [`spawn_starrocks_catalog_wire`]'s `delegation`
/// param skips the bootstrap entirely — [`create_starrocks_engine`] passes
/// `None` when the body's `delegationMode` is omitted (no delegation opted
/// into) and `Some(..)` for either delegation mode, built from
/// `st.engine_jwks_uri`/`engine_oidc_issuer` plus a minted
/// [`crate::delegator_policy::DELEGATOR_USERNAME`] secret.
struct StarRocksDelegationBootstrap {
    jwks_url: String,
    issuer: String,
    audience: String,
    /// `Some(pem)` runs the bootstrap DDL over TLS, pinning this cert (mirrors
    /// the connector's `options.tls_cert`); `None` stays plain TCP.
    tls_cert_pem: Option<String>,
    service_user: String,
    service_password: String,
}

/// Best-effort, non-blocking post-provision step (T6 step 3, extended by
/// S3-T2 step 3): poll the provisioner's `status` until a StarRocks FE pod
/// reports ready (bounded to ~5 minutes), then run
/// [`starrocks_external_catalog_sql`] against the FE's in-cluster
/// MySQL-protocol service, and — when `delegation` is `Some` — the JWT
/// security integration + impersonation service-user bootstrap
/// ([`starrocks_jwt_integration_sql`] / [`starrocks_create_service_user_sql`] /
/// [`starrocks_impersonation_grant_sql`]), over TLS when the bundle carries a
/// cert. The DDL embeds the OAuth2 `client_id:client_secret` credential
/// directly (StarRocks self-refreshes its access token from it, so there is
/// no short-lived bearer to mint here and no token-expiry to recover from).
/// Spawned via `tokio::spawn` so `create_starrocks_engine` returns its HTTP
/// response without waiting on any of this — the FE takes ~1-2 min to come
/// up, and neither the poll budget nor FE/egret reachability in a given
/// cluster is something the create request should block on. Every failure
/// path only logs (`tracing::warn!`); it never has a channel back to the
/// original HTTP response, by construction. The FE service DNS name
/// (`<name>-fe-service.<namespace>.svc`) is the kube-starrocks operator's
/// documented convention; UNVERIFIED against a live operator install as of
/// this change (flagged for the Task 8 / S3-T4 live smoke — see plan doc).
#[allow(clippy::too_many_arguments)]
fn spawn_starrocks_catalog_wire(
    prov: Arc<dyn egret_engine_mgmt_api::EngineProvisioningPort>,
    namespace: String,
    engine_name: String,
    egret_url: String,
    warehouse_name: String,
    client_id: String,
    client_secret: String,
    root_password: Option<String>,
    delegation: Option<StarRocksDelegationBootstrap>,
) {
    tokio::spawn(async move {
        const MAX_POLLS: u32 = 30;
        const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);
        let mut fe_ready = false;
        for _ in 0..MAX_POLLS {
            match prov.status("starrocks", &namespace, &engine_name).await {
                Ok(s) => {
                    if s.pods.iter().any(|p| p.name.contains("-fe-") && p.ready) {
                        fe_ready = true;
                        break;
                    }
                }
                Err(e) => {
                    tracing::debug!(engine = %engine_name, error = %e,
                        "starrocks catalog auto-wire: status poll failed");
                }
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        if !fe_ready {
            tracing::warn!(engine = %engine_name,
                "starrocks catalog auto-wire: FE never reported ready within the poll budget; \
                 skipping — the admin can retry (e.g. re-provision) once it is");
            return;
        }

        // The OAuth2 credential is the SAME `client_id:client_secret` the Trino
        // path stores (see `build_catalog_req`'s egret-warehouse arm); StarRocks
        // exchanges it for an access token itself and refreshes on expiry.
        let credential = format!("{client_id}:{client_secret}");
        let sql = starrocks_external_catalog_sql("egret", &egret_url, &warehouse_name, &credential);
        let fe_host = format!("{engine_name}-fe-service.{namespace}.svc");
        let root_pw = root_password.as_deref();
        match run_starrocks_ddl(&fe_host, STARROCKS_FE_MYSQL_PORT, &sql, root_pw, None).await {
            Ok(()) => {
                tracing::info!(engine = %engine_name,
                    "starrocks catalog auto-wire: external Iceberg catalog wired");
            }
            Err(e) => {
                tracing::warn!(engine = %engine_name, %fe_host, error = %e,
                    "starrocks catalog auto-wire: CREATE EXTERNAL CATALOG failed; admin can retry");
            }
        }

        // Delegation bootstrap (S3-T2 step 3): JWT integration + impersonation
        // service user, best-effort + logged like the catalog wire above.
        if let Some(d) = delegation {
            let mut stmts = starrocks_jwt_integration_sql(&d.jwks_url, &d.issuer, &d.audience);
            stmts.push(starrocks_create_service_user_sql(
                &d.service_user,
                &d.service_password,
            ));
            stmts.push(starrocks_impersonation_grant_sql(&d.service_user));
            let mut all_ok = true;
            for stmt in &stmts {
                if let Err(e) = run_starrocks_ddl(
                    &fe_host,
                    STARROCKS_FE_MYSQL_PORT,
                    stmt,
                    root_pw,
                    d.tls_cert_pem.as_deref(),
                )
                .await
                {
                    all_ok = false;
                    tracing::warn!(engine = %engine_name, %fe_host, error = %e,
                        "starrocks delegation bootstrap: statement failed; admin can retry");
                }
            }
            if all_ok {
                tracing::info!(engine = %engine_name,
                    "starrocks delegation bootstrap: JWT integration + impersonation user wired");
            }
        }
    });
}

#[utoipa::path(
    get,
    path = "/api/control-plane/engines/{id}",
    tag = "control-plane-engines",
    params(("id" = Uuid, Path, description = "Engine id")),
    responses(
        (status = 200, description = "The engine row flattened with live Kubernetes status", body = EngineDetailDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:read permission", body = ErrorBody),
        (status = 404, description = "No such engine", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn get_engine(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<EngineDetailDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:read").await?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    let status = match st.engine_provisioner.as_ref() {
        Some(prov) => match prov
            .status(&engine.r#type, &engine.namespace, &engine.name)
            .await
        {
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

#[utoipa::path(
    put,
    path = "/api/control-plane/engines/{id}",
    tag = "control-plane-engines",
    params(("id" = Uuid, Path, description = "Engine id")),
    request_body = CreateEngineBody,
    responses(
        (status = 200, description = "Engine spec updated and pods rolled", body = EngineDto),
        (status = 400, description = "Invalid config or engine update failed", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:update permission", body = ErrorBody),
        (status = 404, description = "No such engine", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn update_engine(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(b): Json<CreateEngineBody>,
) -> Result<Json<EngineDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:update").await?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    // StarRocks has its own spec builder + provision-req assembler (FE/CN roles,
    // builtin storage volume) — dispatch to it rather than running the Trino
    // spec/assembly below.
    if engine.r#type == "starrocks" {
        return update_starrocks_engine(st, engine, b).await;
    }
    if engine.r#type != "trino" {
        return Err(CpError::BadRequest(
            "update not supported for this engine kind".into(),
        ));
    }

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

/// StarRocks update path: re-persist the FE/CN spec (image/replicas/resources)
/// from the PUT body — omitted fields preserved via [`build_starrocks_spec`] —
/// then re-provision. The primary Catalog Client is NOT re-minted (its
/// credential lives on the primary catalog row), so nothing is revoked here.
///
/// A config-only PUT (no delegation change) just re-provisions the CR:
/// [`build_starrocks_spec`] preserves the existing delegation block. When the
/// delegation mode / TLS actually CHANGES, it additionally re-runs the post-Ready
/// FE bootstrap (JWT integration + impersonation service user) and
/// re-materializes every assigned workspace's connection so the connector picks
/// up the new mode — see the `deleg_changed` branch below. Enforcement of the
/// authz `engine:update` permission already happened in the [`update_engine`]
/// entry point before this dispatch.
async fn update_starrocks_engine(
    st: CpState,
    engine: Engine,
    b: CreateEngineBody,
) -> Result<Json<EngineDto>, CpError> {
    let sr_body = b.starrocks.clone().unwrap_or_default();
    // Validate delegationMode exactly like create (jwt|impersonate|blank) BEFORE
    // persisting anything, so a bad value 400s without corrupting the stored spec.
    validate_sr_delegation_mode(&sr_body)?;
    // Same up-front guard on a `feConfig` override of an egret-managed key.
    validate_sr_fe_config(&sr_body)?;
    // ...and on out-of-range autoscaling bounds (min>max / min==0 / cpu==0),
    // which the operator/HPA would otherwise reject or silently mis-apply. This
    // guard was originally wired into create only; an edit must enforce it too
    // (a config that can't be created must not be reachable via update either).
    validate_sr_autoscaling(&sr_body)?;

    // Capture the OLD delegation block ({mode, tlsEnabled} or null) from the
    // pre-update engine before we overwrite the spec, so we can detect a
    // delegation/TLS change below.
    let old_delegation = sr_delegation_block(&engine.spec);

    // Merge the body over the CURRENT spec so a PUT that omits image/replicas/
    // resources (or delegation) preserves what's already stored.
    let spec = build_starrocks_spec(&sr_body, Some(&engine.spec));
    // A delegation change flips the connector's auth mode (impersonate ⇄ jwt) or
    // its in-band TLS, so it must re-bootstrap the FE. Compare the full stored
    // `delegation` block before vs. after (see `sr_delegation_block`); a spurious
    // match-miss only costs an idempotent no-op re-bootstrap. This gates ONLY the
    // FE re-bootstrap — re-materialization runs unconditionally below.
    let deleg_changed = old_delegation != sr_delegation_block(&spec);
    st.engine_mgmt
        .update_engine(engine.id, &spec, &engine.status)
        .await?;
    let engine = st
        .engine_mgmt
        .get_engine(engine.id)
        .await?
        .ok_or(CpError::NotFound)?;

    // Re-resolve the warehouse (same resolver create uses) so the FE/CN builtin
    // storage volume is fed the SAME S3 coords, and grab the delegation bootstrap
    // (unused today, consumed by the later delegation-change task).
    let wh = st
        .catalog
        .get_warehouse(engine.warehouse_id)
        .await?
        .ok_or_else(|| CpError::BadRequest("unknown warehouse".into()))?;
    let (req, bootstrap) = build_starrocks_provision_req(&st, &engine, &wh).await?;

    let Some(prov) = st.engine_provisioner.as_ref() else {
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };
    if let Err(msg) = prov.update(&req).await {
        return Err(CpError::BadRequest(format!("engine update failed: {msg}")));
    }

    // Two follow-up steps, deliberately DECOUPLED so a partial failure converges
    // on a retry:
    //
    //   (a) FE re-bootstrap — gated on `deleg_changed`. Only a delegation/TLS
    //       change needs the heavier JWT-integration + impersonation-service-user
    //       DDL re-run; an image/replica-only edit must not pay for it.
    //   (b) Re-materialize the per-workspace connections — ALWAYS run, never
    //       gated. It's idempotent + cheap (a no-op when the options are
    //       unchanged), and running it unconditionally is what lets a retried PUT
    //       heal a prior partial re-materialization: `deleg_changed` diffs the
    //       *stored* old spec against the new one, and the new spec is persisted
    //       above BEFORE this runs, so a retry with the same body would see
    //       `old == new` (`deleg_changed = false`). Gating (b) on that flag would
    //       strand any workspaces the first attempt missed on stale creds with no
    //       recovery path — hence: always re-materialize.
    //
    // Eventual-consistency window (mirrors create's posture): (b) updates each
    // workspace's stored connection options synchronously so they advertise the
    // new mode immediately, while (a) is spawned detached and runs against the FE
    // only once it reports Ready (~1-2 min) — so briefly the connections announce
    // a mode the FE has not finished (re)bootstrapping.
    if deleg_changed {
        // (a) Re-bootstrap, mirroring create's non-blocking post-Ready wire.
        // Needs the primary catalog's OAuth2 credential (create embedded it in
        // the primary `engine_catalogs` row); best-effort like create, so a
        // missing credential warns rather than failing the update.
        if let Some(boot) = bootstrap {
            match sr_primary_catalog_credential(&st, engine.id).await {
                Some((client_id, client_secret)) => {
                    // Same minted-root-password threading as create's wire.
                    let root_password =
                        req.starrocks.as_ref().and_then(|s| s.root_password.clone());
                    spawn_starrocks_catalog_wire(
                        prov.clone(),
                        engine.namespace.clone(),
                        engine.name.clone(),
                        st.engine_egret_url.clone(),
                        wh.name.clone(),
                        client_id,
                        client_secret,
                        root_password,
                        Some(boot),
                    );
                }
                None => {
                    tracing::warn!(
                        engine = %engine.name,
                        "delegation changed but no primary catalog credential found; \
                         skipping FE re-bootstrap (re-materialization still runs)"
                    );
                }
            }
        }
    }
    // (b) Re-materialize the per-workspace connections unconditionally. The CR
    // was already updated, so a failure here must surface loudly (400) rather
    // than silently leave stale connection options behind.
    st.assignment
        .rematerialize_for_engine(engine.id)
        .await
        .map_err(|e| {
            CpError::BadRequest(format!(
                "engine updated but re-materializing workspace connections failed: {e}"
            ))
        })?;
    Ok(Json(engine.into()))
}

/// The full `delegation` sub-object of a StarRocks engine spec (today
/// `{mode, tlsEnabled}`), or `Null` when absent. Used to detect a delegation/TLS
/// change across an update: an inequality between the pre- and post-update blocks
/// means the connector's auth mode or in-band TLS flipped. Comparing the WHOLE
/// block (not just `mode`/`tlsEnabled`) is the safe side — at worst a future
/// extra key triggers a spurious FE re-bootstrap, which is an idempotent no-op.
fn sr_delegation_block(spec: &serde_json::Value) -> serde_json::Value {
    spec.get("starrocksConfig")
        .and_then(|c| c.get("delegation"))
        .cloned()
        .unwrap_or(serde_json::Value::Null)
}

/// Recover the OAuth2 `(client_id, client_secret)` create embedded in the
/// engine's PRIMARY `engine_catalogs` row (`config.credential` =
/// `"<client_id>:<client_secret>"`). Returns `None` when there is no primary
/// catalog or the credential is absent/malformed — the re-bootstrap then skips
/// the catalog-wire step (best-effort, like create).
async fn sr_primary_catalog_credential(st: &CpState, engine_id: Uuid) -> Option<(String, String)> {
    let cats = st.engine_mgmt.list_engine_catalogs(engine_id).await.ok()?;
    let primary = cats.iter().find(|c| c.is_primary)?;
    let credential = primary.config.get("credential")?.as_str()?;
    let (client_id, client_secret) = credential.split_once(':')?;
    Some((client_id.to_string(), client_secret.to_string()))
}

#[utoipa::path(
    delete,
    path = "/api/control-plane/engines/{id}",
    tag = "control-plane-engines",
    params(("id" = Uuid, Path, description = "Engine id")),
    responses(
        (status = 204, description = "Engine deprovisioned; catalogs and minted Catalog Clients cleaned up"),
        (status = 400, description = "Engine delete failed on Kubernetes", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:delete permission", body = ErrorBody),
        (status = 404, description = "No such engine", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn delete_engine(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:delete").await?;
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
        prov.delete(&engine.r#type, &engine.namespace, &engine.name)
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

/// `POST /engines/{id}/restart` — roll the engine's pods without changing config
/// (like `kubectl rollout restart`). Unlike `update`, this always forces a roll:
/// `prov.restart` patches a fresh timestamp annotation onto the pod templates
/// regardless of whether the rendered config changed. For Trino this rolls the
/// coordinator + worker Deployments directly; for StarRocks it merge-patches the
/// restart stamp onto the `StarRocksCluster` CR's FE + CN specs and the operator
/// propagates it to the pod templates.
#[utoipa::path(
    post,
    path = "/api/control-plane/engines/{id}/restart",
    tag = "control-plane-engines",
    params(("id" = Uuid, Path, description = "Engine id")),
    responses(
        (status = 200, description = "Coordinator + worker pods rolled (rollout restart)"),
        (status = 400, description = "Kubernetes not configured or restart failed", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:provision permission", body = ErrorBody),
        (status = 404, description = "No such engine", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn restart_engine(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:provision").await?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    // FAIL-FAST on unsupported kinds. Trino rolls its Deployments directly;
    // StarRocks rolls the operator-owned FE/CN pods via a CR annotation merge
    // patch (see `StarRocksProvisioner::restart`). Any other kind has no restart
    // path wired, so reject it rather than silently no-op.
    if engine.r#type != "trino" && engine.r#type != "starrocks" {
        return Err(CpError::BadRequest(format!(
            "restart not supported for {} engines",
            engine.r#type
        )));
    }
    let Some(prov) = st.engine_provisioner.as_ref() else {
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };
    prov.restart(&engine.r#type, &engine.namespace, &engine.name)
        .await
        .map_err(|e| CpError::BadRequest(format!("engine restart failed: {e}")))?;
    Ok(StatusCode::OK)
}

// ── engine ↔ workspace assignment ────────────────────────────────────────────────

/// A workspace an engine is assigned to (CP `…/workspaces` list payload).
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct EngineWorkspaceDto {
    workspace_id: Uuid,
    name: String,
    slug: String,
    assigned_at: chrono::DateTime<chrono::Utc>,
}
impl From<crate::store::EngineWorkspace> for EngineWorkspaceDto {
    fn from(w: crate::store::EngineWorkspace) -> Self {
        EngineWorkspaceDto {
            workspace_id: w.workspace_id,
            name: w.name,
            slug: w.slug,
            assigned_at: w.assigned_at,
        }
    }
}

/// `PUT /engines/{id}/workspaces` body: the FULL desired set of workspace ids the
/// engine should be assigned to (the handler diffs it against the current set).
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct SetWorkspacesBody {
    workspace_ids: Vec<Uuid>,
}

/// An engine assigned to a workspace (workspace-admin `…/engines` list payload).
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct WorkspaceEngineDto {
    id: Uuid,
    name: String,
    #[serde(rename = "type")]
    r#type: String,
    status: String,
}
impl From<Engine> for WorkspaceEngineDto {
    fn from(e: Engine) -> Self {
        WorkspaceEngineDto {
            id: e.id,
            name: e.name,
            r#type: e.r#type,
            status: e.status,
        }
    }
}

/// `POST /workspaces/{id}/engines` body: the engine to assign to the workspace.
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct AssignEngineBody {
    engine_id: Uuid,
}

#[utoipa::path(
    get,
    path = "/api/control-plane/engines/{id}/workspaces",
    tag = "control-plane-engines",
    params(("id" = Uuid, Path, description = "Engine id")),
    responses(
        (status = 200, description = "Workspaces this engine is assigned to", body = Vec<EngineWorkspaceDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:read permission", body = ErrorBody),
        (status = 404, description = "No such engine", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_engine_workspaces(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<EngineWorkspaceDto>>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:read").await?;
    st.engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    let ws = st.engine_mgmt.list_workspaces_for_engine(id).await?;
    Ok(Json(ws.into_iter().map(Into::into).collect()))
}

#[utoipa::path(
    put,
    path = "/api/control-plane/engines/{id}/workspaces",
    tag = "control-plane-engines",
    params(("id" = Uuid, Path, description = "Engine id")),
    request_body = SetWorkspacesBody,
    responses(
        (status = 200, description = "Assignment set replaced; returns the new workspace set", body = Vec<EngineWorkspaceDto>),
        (status = 400, description = "An engine that is not assignable (e.g. no JWT auth)", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:update permission", body = ErrorBody),
        (status = 404, description = "No such engine", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn set_engine_workspaces(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(b): Json<SetWorkspacesBody>,
) -> Result<Json<Vec<EngineWorkspaceDto>>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:update").await?;
    st.engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;

    // Diff the requested set against the current set and apply per-delta. The
    // assignment service keeps the query-seam materialized connection in sync.
    let current: std::collections::HashSet<Uuid> = st
        .engine_mgmt
        .list_workspaces_for_engine(id)
        .await?
        .into_iter()
        .map(|w| w.workspace_id)
        .collect();
    let desired: std::collections::HashSet<Uuid> = b.workspace_ids.into_iter().collect();
    let actor = p.user_id.0;
    for ws in desired.difference(&current) {
        st.assignment.assign(id, *ws, actor).await?;
    }
    for ws in current.difference(&desired) {
        st.assignment.unassign(id, *ws).await?;
    }

    let ws = st.engine_mgmt.list_workspaces_for_engine(id).await?;
    Ok(Json(ws.into_iter().map(Into::into).collect()))
}

#[utoipa::path(
    get,
    path = "/api/admin/workspaces/{id}/engines",
    tag = "control-plane-engines",
    params(("id" = Uuid, Path, description = "Workspace id")),
    responses(
        (status = 200, description = "Engines assigned to this workspace", body = Vec<WorkspaceEngineDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_workspace_engines(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<WorkspaceEngineDto>>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:read").await?;
    let engines = st.engine_mgmt.list_engines_for_workspace(id).await?;
    Ok(Json(engines.into_iter().map(Into::into).collect()))
}

#[utoipa::path(
    post,
    path = "/api/admin/workspaces/{id}/engines",
    tag = "control-plane-engines",
    params(("id" = Uuid, Path, description = "Workspace id")),
    request_body = AssignEngineBody,
    responses(
        (status = 204, description = "Engine assigned to the workspace"),
        (status = 400, description = "Engine is not assignable (e.g. no JWT auth)", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:update permission", body = ErrorBody),
        (status = 404, description = "No such engine", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn assign_workspace_engine(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(b): Json<AssignEngineBody>,
) -> Result<StatusCode, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:update").await?;
    st.assignment.assign(b.engine_id, id, p.user_id.0).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete,
    path = "/api/admin/workspaces/{id}/engines/{engine_id}",
    tag = "control-plane-engines",
    params(
        ("id" = Uuid, Path, description = "Workspace id"),
        ("engine_id" = Uuid, Path, description = "Engine id"),
    ),
    responses(
        (status = 204, description = "Engine unassigned from the workspace"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:update permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn unassign_workspace_engine(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, engine_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:update").await?;
    st.assignment.unassign(engine_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── catalog sub-routes ───────────────────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/api/control-plane/engines/{id}/catalogs",
    tag = "control-plane-engines",
    params(("id" = Uuid, Path, description = "Engine id")),
    responses(
        (status = 200, description = "Catalogs attached to the engine", body = Vec<EngineCatalogDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:read permission", body = ErrorBody),
        (status = 404, description = "No such engine", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_catalogs(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<EngineCatalogDto>>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:read").await?;
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

#[utoipa::path(
    post,
    path = "/api/control-plane/engines/{id}/catalogs",
    tag = "control-plane-engines",
    params(("id" = Uuid, Path, description = "Engine id")),
    request_body = AddCatalogBody,
    responses(
        (status = 201, description = "Catalog added and engine re-provisioned", body = EngineCatalogDto),
        (status = 400, description = "Missing/invalid catalog kind, warehouse, or engine update failed", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:create permission", body = ErrorBody),
        (status = 404, description = "No such engine", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn add_catalog(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(b): Json<AddCatalogBody>,
) -> Result<(StatusCode, Json<EngineCatalogDto>), CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:create").await?;
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

#[utoipa::path(
    get,
    path = "/api/control-plane/engines/{id}/catalogs/{cid}",
    tag = "control-plane-engines",
    params(
        ("id" = Uuid, Path, description = "Engine id"),
        ("cid" = Uuid, Path, description = "Catalog id"),
    ),
    responses(
        (status = 200, description = "Full catalog config (system-minted secret material redacted)", body = EngineCatalogDetailDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:read permission", body = ErrorBody),
        (status = 404, description = "No such engine or catalog (or catalog not on this engine)", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn get_catalog(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, cid)): Path<(Uuid, Uuid)>,
) -> Result<Json<EngineCatalogDetailDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:read").await?;
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
#[derive(Serialize, ToSchema)]
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
#[utoipa::path(
    post,
    path = "/api/control-plane/engines/{id}/catalogs/{cid}/test",
    tag = "control-plane-engines",
    params(
        ("id" = Uuid, Path, description = "Engine id"),
        ("cid" = Uuid, Path, description = "Catalog id"),
    ),
    responses(
        (status = 200, description = "Test-connection result (ok=false with a message on a coordinator/query failure)", body = CatalogTestDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:read permission", body = ErrorBody),
        (status = 404, description = "No such engine or catalog (or catalog not on this engine)", body = ErrorBody),
        (status = 502, description = "Trino coordinator unreachable", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn test_catalog(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, cid)): Path<(Uuid, Uuid)>,
) -> Result<Json<CatalogTestDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:read").await?;
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

#[utoipa::path(
    delete,
    path = "/api/control-plane/engines/{id}/catalogs/{cid}",
    tag = "control-plane-engines",
    params(
        ("id" = Uuid, Path, description = "Engine id"),
        ("cid" = Uuid, Path, description = "Catalog id"),
    ),
    responses(
        (status = 200, description = "Catalog removed and engine re-provisioned"),
        (status = 400, description = "Cannot delete the primary catalog or engine update failed", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:delete permission", body = ErrorBody),
        (status = 404, description = "No such engine or catalog (or catalog not on this engine)", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn remove_catalog(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, cid)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:delete").await?;
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

#[utoipa::path(
    put,
    path = "/api/control-plane/engines/{id}/catalogs/{cid}",
    tag = "control-plane-engines",
    params(
        ("id" = Uuid, Path, description = "Engine id"),
        ("cid" = Uuid, Path, description = "Catalog id"),
    ),
    request_body = UpdateCatalogBody,
    responses(
        (status = 200, description = "Catalog config replaced and engine re-provisioned", body = EngineCatalogDto),
        (status = 400, description = "Primary or managed egret-warehouse catalogs are not editable, or engine update failed", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:update permission", body = ErrorBody),
        (status = 404, description = "No such engine or catalog (or catalog not on this engine)", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn update_catalog(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    Json(b): Json<UpdateCatalogBody>,
) -> Result<Json<EngineCatalogDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:update").await?;
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

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct PodLogsDto {
    logs: String,
}

/// `GET /engines/{id}/pods/{pod}/logs?tail=&previous=&container=` — bounded tail
/// of a pod's logs. The pod must belong to the engine (verified against the live
/// pod list from `EngineProvisioner::status`), so one platform admin can't read
/// an arbitrary pod's logs by guessing a name.
#[utoipa::path(
    get,
    path = "/api/control-plane/engines/{id}/pods/{pod}/logs",
    tag = "control-plane-engines",
    params(
        ("id" = Uuid, Path, description = "Engine id"),
        ("pod" = String, Path, description = "Pod name (must belong to the engine)"),
        ("tail" = Option<i64>, Query, description = "Number of trailing lines (1..=5000, default 1000)"),
        ("previous" = Option<bool>, Query, description = "Read the previous (crashed) container instance"),
        ("container" = Option<String>, Query, description = "Container name (defaults to the pod's main container)"),
    ),
    responses(
        (status = 200, description = "Bounded tail of the pod's logs", body = PodLogsDto),
        (status = 400, description = "Kubernetes not configured or log read failed", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:read permission", body = ErrorBody),
        (status = 404, description = "No such engine, or pod does not belong to the engine", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn pod_logs(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, pod)): Path<(Uuid, String)>,
    axum::extract::Query(q): axum::extract::Query<PodLogsQuery>,
) -> Result<Json<PodLogsDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:read").await?;
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
        .status(&engine.r#type, &engine.namespace, &engine.name)
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
#[utoipa::path(
    get,
    path = "/api/control-plane/engines/{id}/pods/{pod}/logs/follow",
    tag = "control-plane-engines",
    params(
        ("id" = Uuid, Path, description = "Engine id"),
        ("pod" = String, Path, description = "Pod name (must belong to the engine)"),
        ("tail" = Option<i64>, Query, description = "Number of trailing lines to backfill (1..=5000, default 1000)"),
    ),
    responses(
        // Not a REST/JSON call: the frontend opens a WebSocket (PodLogs) and reads
        // log byte chunks as Binary frames; there is no JSON response body. The
        // error statuses below are returned BEFORE the upgrade.
        (status = 101, description = "WebSocket upgrade streaming the pod's logs (kubectl logs -f equivalent)"),
        (status = 400, description = "Kubernetes not configured", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:read permission", body = ErrorBody),
        (status = 404, description = "No such engine, or pod does not belong to the engine", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn pod_logs_follow(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, pod)): Path<(Uuid, String)>,
    axum::extract::Query(q): axum::extract::Query<PodLogsFollowQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:read").await?;
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
        .status(&engine.r#type, &engine.namespace, &engine.name)
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
#[utoipa::path(
    get,
    path = "/api/control-plane/engines/{id}/pods/{pod}/exec",
    tag = "control-plane-engines",
    params(
        ("id" = Uuid, Path, description = "Engine id"),
        ("pod" = String, Path, description = "Pod name (must belong to the engine)"),
    ),
    responses(
        // Not a REST/JSON call: the frontend opens a WebSocket (PodTerminal) and
        // bridges it bidirectionally to a `/bin/sh` session in the pod (stdin +
        // resize control frames up, stdout Binary frames down). No JSON body. The
        // error statuses below are returned BEFORE the upgrade.
        (status = 101, description = "WebSocket upgrade bridging an interactive exec session in the pod"),
        (status = 400, description = "Kubernetes not configured", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:provision permission", body = ErrorBody),
        (status = 404, description = "No such engine, or pod does not belong to the engine", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn pod_exec(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, pod)): Path<(Uuid, String)>,
    ws: WebSocketUpgrade,
) -> Result<Response, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:provision").await?;
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
        .status(&engine.r#type, &engine.namespace, &engine.name)
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

/// OpenAPI fragment for the engine-provisioning API (engines CRUD + lifecycle,
/// catalog auto-wiring + management, and the pod logs/exec bridges). Nested at
/// `/api/control-plane` in `egret-server`, so every `#[utoipa::path]` above
/// carries the FULL mounted path. The native-Trino monitor routes merged into
/// the same router carry their own [`crate::monitor::MonitorApi`] fragment,
/// unioned in `egret_engine_mgmt::openapi()`. Errors use the shared
/// [`egret_core::http::ErrorBody`]. No schema name here collides with the merged
/// doc's other seams (verified against the committed `openapi.json`).
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        list_engines,
        create_engine,
        get_engine,
        update_engine,
        delete_engine,
        restart_engine,
        list_engine_workspaces,
        set_engine_workspaces,
        list_workspace_engines,
        assign_workspace_engine,
        unassign_workspace_engine,
        list_catalogs,
        add_catalog,
        get_catalog,
        test_catalog,
        remove_catalog,
        update_catalog,
        pod_logs,
        pod_logs_follow,
        pod_exec,
    ),
    components(schemas(
        CreateEngineBody,
        EngineSpecDto,
        EngineDto,
        EngineDetailDto,
        EngineStatusDto,
        EnginePodDto,
        EngineCatalogDto,
        EngineCatalogDetailDto,
        AddCatalogBody,
        UpdateCatalogBody,
        CatalogTestDto,
        PodLogsDto,
        EngineWorkspaceDto,
        SetWorkspacesBody,
        WorkspaceEngineDto,
        AssignEngineBody,
        ErrorBody,
    )),
    tags((name = "control-plane-engines", description = "Engine provisioning: Trino-on-Kubernetes clusters, their catalogs, native query monitor, and pod logs/exec")),
)]
pub struct EngineMgmtApi;

#[cfg(test)]
mod tests {
    use super::*;

    /// Allow-all PDP for these unit tests: they exercise catalog rendering /
    /// provisioning-request building, not the authz gate, so the decision point
    /// is incidental.
    struct AllowAllPdp;
    #[async_trait::async_trait]
    impl egret_core::authz::Pdp for AllowAllPdp {
        async fn authorize(
            &self,
            _s: &egret_core::authz::Subject,
            _a: &str,
            _r: &egret_core::authz::Resource,
        ) -> egret_core::Result<egret_core::authz::Decision> {
            Ok(egret_core::authz::Decision {
                allow: true,
                reasons: vec![],
            })
        }
    }

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

    /// Fix #3c: enabling the engine `jwt` block with no JWKS anywhere (platform
    /// OIDC unconfigured AND no per-engine `jwksUri`) must be REJECTED at the
    /// API boundary, not silently provision a coordinator that crashes at boot.
    /// Self-skips without `DATABASE_URL` (mirrors the other `test_state` tests).
    #[tokio::test]
    async fn resolve_auth_rejects_jwt_when_no_jwks() {
        let Some(mut st) = test_state().await else {
            eprintln!("skipping resolve_auth_rejects_jwt_when_no_jwks: DATABASE_URL not set");
            return;
        };

        // Platform has no JWKS (OIDC unconfigured).
        st.engine_jwks_uri.clear();

        // jwt block enabled + no jwks anywhere → BadRequest.
        let tc = serde_json::json!({ "auth": { "jwt": {} } });
        let err = resolve_auth(&st, &tc)
            .await
            .expect_err("jwt with no jwks must be rejected");
        assert!(matches!(err, CpError::BadRequest(_)), "got {err:?}");

        // An explicit per-engine jwksUri satisfies it even with no platform JWKS.
        let tc_explicit =
            serde_json::json!({ "auth": { "jwt": { "jwksUri": "https://engine/jwks" } } });
        let out = resolve_auth(&st, &tc_explicit)
            .await
            .expect("explicit per-engine jwks is accepted");
        assert!(out.and_then(|a| a.jwt).is_some());

        // No jwt block (password-only / no-security) is unaffected by the gate.
        let tc_no_jwt = serde_json::json!({ "auth": { "passwordUsers": [] } });
        let out = resolve_auth(&st, &tc_no_jwt)
            .await
            .expect("no-jwt auth is accepted");
        assert!(out.expect("auth block present").jwt.is_none());

        // With a platform JWKS present, the jwt block is accepted and falls back
        // to the platform default.
        st.engine_jwks_uri = "https://idp/jwks".into();
        let tc2 = serde_json::json!({ "auth": { "jwt": {} } });
        let jwt = resolve_auth(&st, &tc2)
            .await
            .expect("platform jwks accepted")
            .and_then(|a| a.jwt)
            .expect("jwt present");
        assert_eq!(jwt.jwks_uri, "https://idp/jwks");
    }

    /// An engine whose stored spec selects a given delegation mode (+ jwt auth so
    /// jwt mode qualifies as delegation-capable).
    fn engine_with_mode(mode: &str) -> Engine {
        let mut e = dummy_engine();
        e.spec = serde_json::json!({
            "trinoConfig": { "delegationMode": mode, "auth": { "jwt": {} } }
        });
        e
    }

    #[test]
    fn provision_branch_gates_tls_delegator_and_monitor_by_mode() {
        // impersonate: delegation-capable, TLS provisioned, delegator injected,
        // monitor injected.
        let imp = engine_with_mode("impersonate");
        assert!(imp.supports_delegation()); // ⇒ TLS provisioned
        assert!(imp.is_impersonate_delegation()); // ⇒ delegator + delegator policies
        assert!(!imp.is_jwt_delegation()); // ⇒ monitor NOT skipped

        // jwt: delegation-capable (jwt auth present), TLS provisioned, but NO
        // delegator (impersonate-only) and monitor SKIPPED (JWT-only coordinator).
        let jwt = engine_with_mode("jwt");
        assert!(jwt.supports_delegation()); // ⇒ TLS provisioned
        assert!(!jwt.is_impersonate_delegation()); // ⇒ no delegator user/policies
        assert!(jwt.is_jwt_delegation()); // ⇒ monitor skipped

        // a jwt engine with NO jwt auth is NOT delegation-capable ⇒ no TLS.
        let mut jwt_no_auth = dummy_engine();
        jwt_no_auth.spec = serde_json::json!({ "trinoConfig": { "delegationMode": "jwt" } });
        assert!(!jwt_no_auth.supports_delegation());
    }

    #[test]
    fn extract_leading_cert_pulls_first_cert_block() {
        let keystore = "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n\
                        -----BEGIN PRIVATE KEY-----\nBBBB\n-----END PRIVATE KEY-----\n";
        let cert = extract_leading_cert(keystore).expect("cert extracted");
        assert!(cert.contains("BEGIN CERTIFICATE"));
        assert!(cert.trim_end().ends_with("-----END CERTIFICATE-----"));
        assert!(!cert.contains("PRIVATE KEY"));
        // No cert block ⇒ None.
        assert!(extract_leading_cert("-----BEGIN PRIVATE KEY-----\nx\n").is_none());
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

    // ── build_starrocks_spec: omitted-field-preserves + delegation rule ─────
    #[test]
    fn build_starrocks_spec_preserves_omitted_fields_on_update() {
        // A fully-populated existing spec (delegation on, custom sizing).
        let existing = serde_json::json!({
            "imageVersion": "starrocks/fe-ubuntu:v1",
            "starrocksConfig": {
                "feImage": "starrocks/fe-ubuntu:v1",
                "cnImage": "starrocks/cn-ubuntu:v1",
                "feReplicas": 3,
                "cnReplicas": 5,
                "feResources": { "memRequest": "4Gi" },
                "cnResources": { "memRequest": "8Gi" },
                "delegation": { "mode": "jwt", "tlsEnabled": true },
            },
        });

        // Config-tab PUT: only feReplicas changes; every other field (incl. the
        // delegation block, which the Config tab does NOT edit) is preserved.
        let body = StarRocksConfigBody {
            fe_replicas: Some(2),
            ..Default::default()
        };
        let spec = build_starrocks_spec(&body, Some(&existing));
        let cfg = &spec["starrocksConfig"];
        assert_eq!(cfg["feReplicas"], 2);
        assert_eq!(cfg["cnReplicas"], 5);
        assert_eq!(cfg["feImage"], "starrocks/fe-ubuntu:v1");
        assert_eq!(cfg["cnImage"], "starrocks/cn-ubuntu:v1");
        assert_eq!(cfg["feResources"]["memRequest"], "4Gi");
        assert_eq!(cfg["cnResources"]["memRequest"], "8Gi");
        assert_eq!(spec["imageVersion"], "starrocks/fe-ubuntu:v1");
        // Delegation NOT dropped by an unrelated config edit.
        assert_eq!(cfg["delegation"]["mode"], "jwt");
        assert_eq!(cfg["delegation"]["tlsEnabled"], true);

        // Create-shaped call (existing = None), empty body ⇒ all create defaults,
        // delegation null.
        let created = build_starrocks_spec(&StarRocksConfigBody::default(), None);
        let ccfg = &created["starrocksConfig"];
        assert_eq!(ccfg["feImage"], DEFAULT_SR_FE_IMAGE);
        assert_eq!(ccfg["cnImage"], DEFAULT_SR_CN_IMAGE);
        assert_eq!(ccfg["feReplicas"], 1);
        assert_eq!(ccfg["cnReplicas"], 1);
        assert!(ccfg["feResources"].is_null());
        assert!(ccfg["cnResources"].is_null());
        assert!(ccfg["delegation"].is_null());

        // Security-tab PUT: a non-blank delegationMode REWRITES the block
        // (tlsEnabled honoured, here opting out).
        let sec = StarRocksConfigBody {
            delegation_mode: Some("impersonate".into()),
            tls_enabled: Some(false),
            ..Default::default()
        };
        let updated = build_starrocks_spec(&sec, Some(&existing));
        assert_eq!(
            updated["starrocksConfig"]["delegation"]["mode"],
            "impersonate"
        );
        assert_eq!(
            updated["starrocksConfig"]["delegation"]["tlsEnabled"],
            false
        );
    }

    // ── build_starrocks_spec: deep-config fields (feConfig/cnConfig/
    //    cnAutoscaling/pod) round-trip + omit=preserve ────────────────────────
    #[test]
    fn build_starrocks_spec_roundtrips_deep_fields() {
        // Create-shaped call: body carries the deep-config surface.
        let mut fe_config = std::collections::BTreeMap::new();
        fe_config.insert("sys_log_level".to_string(), "WARN".to_string());
        let body = StarRocksConfigBody {
            fe_config: Some(fe_config),
            cn_autoscaling: Some(serde_json::json!({ "maxReplicas": 3 })),
            pod: Some(serde_json::json!({ "nodeSelector": { "disktype": "ssd" } })),
            ..Default::default()
        };
        let created = build_starrocks_spec(&body, None);
        let cfg = &created["starrocksConfig"];
        assert_eq!(cfg["feConfig"]["sys_log_level"], "WARN");
        assert_eq!(cfg["cnAutoscaling"]["maxReplicas"], 3);
        assert_eq!(cfg["pod"]["nodeSelector"]["disktype"], "ssd");

        // Follow-up update that OMITS the deep fields ⇒ all preserved verbatim.
        let update = StarRocksConfigBody {
            fe_replicas: Some(2),
            ..Default::default()
        };
        let updated = build_starrocks_spec(&update, Some(&created));
        let ucfg = &updated["starrocksConfig"];
        assert_eq!(ucfg["feReplicas"], 2);
        assert_eq!(ucfg["feConfig"]["sys_log_level"], "WARN");
        assert_eq!(ucfg["cnAutoscaling"]["maxReplicas"], 3);
        assert_eq!(ucfg["pod"]["nodeSelector"]["disktype"], "ssd");
    }

    #[test]
    fn engine_spec_dto_surfaces_starrocks_config() {
        // REGRESSION (caught by the full live smoke): `EngineSpecDto` only
        // carried `trinoConfig`, so `From<Engine>`'s `from_value` silently
        // dropped `starrocksConfig` — the UI then read `undefined` and showed
        // empty replicas/auth + an un-seeded config editor for every StarRocks
        // engine. The DTO must round-trip the stored `starrocksConfig` back out.
        let stored_spec = serde_json::json!({
            "imageVersion": "starrocks/fe-ubuntu:3.4-latest",
            "starrocksConfig": {
                "feImage": "starrocks/fe-ubuntu:3.4-latest",
                "feReplicas": 1,
                "cnReplicas": 1,
                "delegation": { "mode": "jwt", "tlsEnabled": true },
                "feConfig": { "sys_log_level": "WARN" },
            },
        });
        let dto: EngineSpecDto = serde_json::from_value(stored_spec).unwrap();
        let out = serde_json::to_value(&dto).unwrap();
        let sc = &out["starrocksConfig"];
        assert!(
            !sc.is_null(),
            "starrocksConfig must be surfaced back to the UI"
        );
        assert_eq!(sc["feReplicas"], 1);
        assert_eq!(sc["cnReplicas"], 1);
        assert_eq!(sc["delegation"]["mode"], "jwt");
        assert_eq!(sc["feConfig"]["sys_log_level"], "WARN");
        // A Trino engine (no starrocksConfig) must NOT emit the key.
        let trino: EngineSpecDto =
            serde_json::from_value(serde_json::json!({ "imageVersion": "x" })).unwrap();
        assert!(serde_json::to_value(&trino)
            .unwrap()
            .get("starrocksConfig")
            .is_none());
    }

    // ── validate_sr_fe_config: protected fe.conf keys are rejected ───────────
    #[test]
    fn feconfig_protected_key_rejected() {
        let mut fe_config = std::collections::BTreeMap::new();
        fe_config.insert("run_mode".to_string(), "shared_nothing".to_string());
        let body = StarRocksConfigBody {
            fe_config: Some(fe_config),
            ..Default::default()
        };
        match validate_sr_fe_config(&body) {
            Err(CpError::BadRequest(m)) => assert!(m.contains("run_mode"), "msg was: {m}"),
            other => panic!("expected BadRequest rejecting run_mode, got {other:?}"),
        }

        // A non-protected key passes.
        let mut ok_config = std::collections::BTreeMap::new();
        ok_config.insert("sys_log_level".to_string(), "WARN".to_string());
        let ok_body = StarRocksConfigBody {
            fe_config: Some(ok_config),
            ..Default::default()
        };
        assert!(validate_sr_fe_config(&ok_body).is_ok());
    }

    // ── validate_sr_autoscaling: enabled-policy bounds are rejected ──────────
    #[test]
    fn autoscaling_bounds_rejected() {
        let body = |a: serde_json::Value| StarRocksConfigBody {
            cn_autoscaling: Some(a),
            ..Default::default()
        };
        let is_bad = |b: StarRocksConfigBody| {
            matches!(validate_sr_autoscaling(&b), Err(CpError::BadRequest(_)))
        };

        // Enabled but no/zero target CPU ⇒ 400.
        assert!(is_bad(body(
            serde_json::json!({ "maxReplicas": 3, "targetCpuPercent": 0 })
        )));
        assert!(is_bad(body(serde_json::json!({ "maxReplicas": 3 })))); // cpu absent
                                                                        // min > max ⇒ 400.
        assert!(is_bad(body(serde_json::json!({
            "maxReplicas": 3, "minReplicas": 5, "targetCpuPercent": 70
        }))));
        // min == 0 ⇒ 400.
        assert!(is_bad(body(serde_json::json!({
            "maxReplicas": 3, "minReplicas": 0, "targetCpuPercent": 70
        }))));

        // Disabled sentinel (max == 0) ⇒ OK (skipped).
        assert!(validate_sr_autoscaling(&body(serde_json::json!({ "maxReplicas": 0 }))).is_ok());
        // Fully valid enabled policy ⇒ OK.
        assert!(validate_sr_autoscaling(&body(serde_json::json!({
            "maxReplicas": 3, "minReplicas": 1, "targetCpuPercent": 70
        })))
        .is_ok());
        // cpu > 100 is valid in k8s ⇒ OK.
        assert!(validate_sr_autoscaling(&body(serde_json::json!({
            "maxReplicas": 3, "targetCpuPercent": 150
        })))
        .is_ok());
        // No autoscaling block at all ⇒ OK.
        assert!(validate_sr_autoscaling(&StarRocksConfigBody::default()).is_ok());
    }

    // ── sr_delegation_block: the delegation-change detector ─────────────────

    #[test]
    fn sr_delegation_block_detects_mode_and_tls_changes() {
        let impersonate = serde_json::json!({
            "starrocksConfig": { "delegation": { "mode": "impersonate", "tlsEnabled": true } }
        });
        let jwt = serde_json::json!({
            "starrocksConfig": { "delegation": { "mode": "jwt", "tlsEnabled": true } }
        });
        let impersonate_no_tls = serde_json::json!({
            "starrocksConfig": { "delegation": { "mode": "impersonate", "tlsEnabled": false } }
        });
        let none = serde_json::json!({ "starrocksConfig": {} });

        // Absent delegation ⇒ Null; a config-only edit that preserves the block
        // is NOT a change.
        assert_eq!(sr_delegation_block(&none), serde_json::Value::Null);
        assert_eq!(
            sr_delegation_block(&impersonate),
            sr_delegation_block(&impersonate)
        );

        // Mode flip, TLS flip, and opting delegation on/off are all changes.
        assert_ne!(sr_delegation_block(&impersonate), sr_delegation_block(&jwt));
        assert_ne!(
            sr_delegation_block(&impersonate),
            sr_delegation_block(&impersonate_no_tls)
        );
        assert_ne!(
            sr_delegation_block(&none),
            sr_delegation_block(&impersonate)
        );
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
            delegator_secret_id: None,
            root_secret_id: None,
            tls_secret_id: None,
            coordinator_cert: None,
            tls_cert_secret_ref: None,
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
        let workspace_secrets =
            egret_secrets::WorkspaceSecretStore::new(db.clone(), secrets.clone());
        let databases =
            egret_query::DatabaseRepo::new(db.clone(), secrets, workspace_secrets.clone());
        Some(CpState {
            engine_mgmt: crate::EngineMgmtStore::new(db.clone()),
            assignment: crate::EngineAssignmentService::new(
                crate::EngineMgmtStore::new(db.clone()),
                databases,
            ),
            catalog: std::sync::Arc::new(egret_catalog_engine::CatalogStore::new(db.clone())),
            storage_registry: egret_storage::StorageRegistry::new(db.clone()),
            authz: std::sync::Arc::new(egret_policy::PolicyStore::new(db.clone())),
            secrets: workspace_secrets,
            sessions: egret_auth::session::SessionService::new(
                db.clone(),
                chrono::Duration::hours(24),
                chrono::Duration::minutes(60),
            ),
            pdp: std::sync::Arc::new(AllowAllPdp),
            engine_provisioner: None,
            kube: None,
            engine_egret_url: "http://egret:8080".into(),
            engine_jwks_uri: "https://idp/jwks".into(),
            engine_oidc_issuer: "https://idp/realms/egret".into(),
            ranger_enabled: false,
        })
    }

    /// Seed a workspace + storage-conn (with a resolvable secret) + warehouse,
    /// returning the `(Warehouse, storage_secret_plaintext)` so a test can drive
    /// `build_starrocks_provision_req`. Mirrors `tests/store.rs::seed_warehouse`
    /// but resolves the concrete `CatalogStore` for the warehouse insert and
    /// backs the storage secret with a real `secrets` row so the helper's
    /// `resolve_plaintext` succeeds.
    async fn seed_sr_warehouse(
        st: &CpState,
        db: &egret_metastore::Db,
    ) -> (egret_catalog_api::Warehouse, String) {
        let catalog = egret_catalog_engine::CatalogStore::new(db.clone());
        let secret_plain = format!("s3-{}", Uuid::new_v4().simple());
        let secret_id = st
            .secrets
            .create(
                None,
                &format!("sr_store_{}", Uuid::new_v4().simple()),
                None,
                &secret_plain,
            )
            .await
            .expect("create storage secret");
        let sc = st
            .storage_registry
            .create_storage_conn(&egret_storage::StorageConnInput {
                name: format!("sr-store-{}", Uuid::new_v4().simple()),
                purpose: None,
                provider: "s3".into(),
                endpoint: Some("http://minio:9000".into()),
                region: "us-east-1".into(),
                bucket: "warehouse".into(),
                key_prefix: None,
                access_key: "egret-ak".into(),
                secret_ref: Some(secret_id),
                path_style: true,
            })
            .await
            .expect("create storage conn");

        let ws_id = Uuid::new_v4();
        let ws_slug = format!("ws_{}", ws_id.simple());
        sqlx::query(
            "INSERT INTO workspaces (id, slug, name, status) VALUES ($1, $2, $3, 'active')",
        )
        .bind(ws_id)
        .bind(&ws_slug)
        .bind(&ws_slug)
        .execute(&db.pool)
        .await
        .expect("seed workspace");
        let wh_name = format!("sr-wh-{}", Uuid::new_v4().simple());
        let wh_row = catalog
            .create_warehouse(&wh_name, ws_id, sc.id)
            .await
            .expect("create warehouse");
        let wh = st
            .catalog
            .get_warehouse(wh_row.id)
            .await
            .expect("get_warehouse")
            .expect("warehouse present");
        (wh, secret_plain)
    }

    /// `build_starrocks_provision_req` (Task 1) rebuilds the provisioner
    /// `EngineSpec` from a STORED engine row's `spec.starrocksConfig` + the
    /// warehouse storage — the shared assembly create and (later) update both
    /// feed the provisioner. Asserts the images/replicas/storage flow through,
    /// `storage_path` follows the `<bucket>/starrocks/<engine>` formula, and the
    /// delegation bundle is present iff the stored spec carries a delegation
    /// block.
    #[tokio::test]
    async fn build_starrocks_provision_req_assembles_from_stored_spec() {
        let Some(st) = test_state().await else {
            eprintln!(
                "skipping build_starrocks_provision_req_assembles_from_stored_spec: DATABASE_URL not set"
            );
            return;
        };
        let dsn = std::env::var("DATABASE_URL").unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (wh, secret_plain) = seed_sr_warehouse(&st, &db).await;

        // (a) A DELEGATION-enabled StarRocks engine row.
        let spec = serde_json::json!({
            "imageVersion": "starrocks/fe-ubuntu:custom",
            "starrocksConfig": {
                "feImage": "starrocks/fe-ubuntu:custom",
                "cnImage": "starrocks/cn-ubuntu:custom",
                "feReplicas": 3,
                "cnReplicas": 5,
                "feResources": null,
                "cnResources": null,
                "delegation": { "mode": "jwt", "tlsEnabled": true },
            },
        });
        let engine = st
            .engine_mgmt
            .create_engine(
                &format!("sr-{}", Uuid::new_v4().simple()),
                "starrocks",
                "egret-engines",
                wh.id,
                &spec,
                None,
            )
            .await
            .expect("create engine row");

        let (req, boot) = build_starrocks_provision_req(&st, &engine, &wh)
            .await
            .expect("build_starrocks_provision_req");

        assert_eq!(req.kind, egret_engine_mgmt_api::EngineKind::StarRocks);
        assert_eq!(req.name, engine.name);
        let sr = req.starrocks.as_ref().expect("starrocks config present");
        assert_eq!(sr.fe_image, "starrocks/fe-ubuntu:custom");
        assert_eq!(sr.cn_image, "starrocks/cn-ubuntu:custom");
        assert_eq!(sr.fe_replicas, 3);
        assert_eq!(sr.cn_replicas, 5);
        assert_eq!(sr.storage_endpoint, "http://minio:9000");
        assert_eq!(sr.storage_access_key, "egret-ak");
        assert_eq!(sr.storage_secret_key, secret_plain);
        assert_eq!(
            sr.storage_path,
            format!("warehouse/starrocks/{}", engine.name)
        );
        // Delegation block present in the stored spec ⇒ delegation + bootstrap.
        assert!(sr.delegation.is_some());
        assert!(boot.is_some());

        // (b) A NO-DELEGATION engine row: images default, delegation absent.
        let spec2 = serde_json::json!({
            "starrocksConfig": {
                "feReplicas": 2,
            },
        });
        let engine2 = st
            .engine_mgmt
            .create_engine(
                &format!("sr-{}", Uuid::new_v4().simple()),
                "starrocks",
                "egret-engines",
                wh.id,
                &spec2,
                None,
            )
            .await
            .expect("create engine row 2");
        let (req2, boot2) = build_starrocks_provision_req(&st, &engine2, &wh)
            .await
            .expect("build_starrocks_provision_req (no delegation)");
        let sr2 = req2.starrocks.as_ref().expect("starrocks config present");
        assert_eq!(sr2.fe_image, DEFAULT_SR_FE_IMAGE);
        assert_eq!(sr2.cn_image, DEFAULT_SR_CN_IMAGE);
        assert_eq!(sr2.fe_replicas, 2);
        assert_eq!(sr2.cn_replicas, 1);
        assert!(sr2.delegation.is_none());
        assert!(boot2.is_none());
    }

    /// A no-op `EngineProvisioningPort` so the full `create_starrocks_engine`
    /// handler runs to completion in tests (which otherwise short-circuit with
    /// "kubernetes not configured" and roll the engine row back). Every method
    /// succeeds; `status` reports no pods (the best-effort catalog-wire task the
    /// handler spawns then just polls in the background and is aborted at test
    /// end).
    struct NoopProvisioner;
    #[async_trait::async_trait]
    impl egret_engine_mgmt_api::EngineProvisioningPort for NoopProvisioner {
        async fn provision(&self, _req: &egret_engine_mgmt_api::EngineSpec) -> Result<(), String> {
            Ok(())
        }
        async fn status(
            &self,
            _kind: &str,
            _namespace: &str,
            _name: &str,
        ) -> Result<egret_engine_mgmt_api::EngineStatus, String> {
            Ok(egret_engine_mgmt_api::EngineStatus::default())
        }
        async fn update(&self, _req: &egret_engine_mgmt_api::EngineSpec) -> Result<(), String> {
            Ok(())
        }
        async fn delete(&self, _kind: &str, _namespace: &str, _name: &str) -> Result<(), String> {
            Ok(())
        }
        async fn restart(&self, _kind: &str, _namespace: &str, _name: &str) -> Result<(), String> {
            Ok(())
        }
    }

    /// `ensure_root_password_secret` is idempotent: two calls (the second on a
    /// row that now carries the persisted `root_secret_id`) return the SAME
    /// secret id and the SAME plaintext, and the first call persists the id.
    #[tokio::test]
    async fn ensure_root_password_secret_is_idempotent() {
        let Some(st) = test_state().await else {
            eprintln!("skipping ensure_root_password_secret_is_idempotent: DATABASE_URL not set");
            return;
        };
        let dsn = std::env::var("DATABASE_URL").unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (wh, _secret) = seed_sr_warehouse(&st, &db).await;
        let engine = st
            .engine_mgmt
            .create_engine(
                &format!("sr-{}", Uuid::new_v4().simple()),
                "starrocks",
                "egret-engines",
                wh.id,
                &serde_json::json!({ "starrocksConfig": {} }),
                None,
            )
            .await
            .expect("create engine row");
        // Fresh row: no root secret yet.
        assert!(engine.root_secret_id.is_none());

        let (id1, pw1) = ensure_root_password_secret(&st, &engine)
            .await
            .expect("mint root password");
        // First call persisted the id on the row.
        let refetched = st
            .engine_mgmt
            .get_engine(engine.id)
            .await
            .expect("get_engine")
            .expect("engine present");
        assert_eq!(refetched.root_secret_id, Some(id1));

        // Second call on the persisted row takes the fast path → same id + plaintext.
        let (id2, pw2) = ensure_root_password_secret(&st, &refetched)
            .await
            .expect("resolve root password");
        assert_eq!(id1, id2);
        assert_eq!(pw1, pw2);
        assert!(!pw1.is_empty());
    }

    /// The projection primitive resolves a `kubernetes.io/tls` named secret to
    /// its `(tls.crt, tls.key)` PEM pair.
    #[tokio::test]
    async fn resolve_tls_cert_secret_returns_crt_and_key() {
        let Some(st) = test_state().await else {
            eprintln!("skipping resolve_tls_cert_secret_returns_crt_and_key: DATABASE_URL not set");
            return;
        };
        let mut data = std::collections::BTreeMap::new();
        data.insert("tls.crt".to_string(), "CERTPEM".to_string());
        data.insert("tls.key".to_string(), "KEYPEM".to_string());
        let id = st
            .secrets
            .create_typed(
                None,
                &format!("tls-{}", Uuid::new_v4().simple()),
                None,
                egret_secrets::secret_type::TLS,
                &data,
            )
            .await
            .expect("create typed tls secret");

        let (crt, key) = resolve_tls_cert_secret(&st, id)
            .await
            .expect("resolve tls cert secret");
        assert_eq!(crt, "CERTPEM");
        assert_eq!(key, "KEYPEM");
    }

    /// A non-TLS (Opaque) secret is rejected with a 400.
    #[tokio::test]
    async fn resolve_tls_cert_secret_rejects_non_tls_type() {
        let Some(st) = test_state().await else {
            eprintln!(
                "skipping resolve_tls_cert_secret_rejects_non_tls_type: DATABASE_URL not set"
            );
            return;
        };
        // A single-value create is an Opaque secret, not kubernetes.io/tls.
        let id = st
            .secrets
            .create(
                None,
                &format!("op-{}", Uuid::new_v4().simple()),
                None,
                "plain",
            )
            .await
            .expect("create opaque secret");
        let err = resolve_tls_cert_secret(&st, id).await.unwrap_err();
        assert!(
            matches!(err, CpError::BadRequest(_)),
            "expected BadRequest, got {err:?}"
        );
    }

    /// A TLS secret whose `tls.key` data row is missing (a corrupt keystore;
    /// `validate()` blocks creating one, so we drop the row) is rejected with a
    /// 400 by the projection's completeness guard.
    #[tokio::test]
    async fn resolve_tls_cert_secret_rejects_missing_key() {
        let Some(st) = test_state().await else {
            eprintln!("skipping resolve_tls_cert_secret_rejects_missing_key: DATABASE_URL not set");
            return;
        };
        let dsn = std::env::var("DATABASE_URL").unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let mut data = std::collections::BTreeMap::new();
        data.insert("tls.crt".to_string(), "CERTPEM".to_string());
        data.insert("tls.key".to_string(), "KEYPEM".to_string());
        let id = st
            .secrets
            .create_typed(
                None,
                &format!("tls-{}", Uuid::new_v4().simple()),
                None,
                egret_secrets::secret_type::TLS,
                &data,
            )
            .await
            .expect("create typed tls secret");
        let vault_id: Uuid =
            sqlx::query_scalar("SELECT secret_ref FROM workspace_secrets WHERE id = $1")
                .bind(id)
                .fetch_one(&db.pool)
                .await
                .expect("vault id");
        sqlx::query("DELETE FROM secret_data WHERE secret_id = $1 AND key = 'tls.key'")
            .bind(vault_id)
            .execute(&db.pool)
            .await
            .expect("drop tls.key row");

        let err = resolve_tls_cert_secret(&st, id).await.unwrap_err();
        assert!(
            matches!(err, CpError::BadRequest(_)),
            "expected BadRequest, got {err:?}"
        );
    }

    /// End-to-end of Task 3: an engine that references a `kubernetes.io/tls`
    /// secret materializes its keystore from the PROVIDED cert (not a fresh
    /// self-signed one) and pins `coordinator_cert` to the provided `tls.crt`.
    #[tokio::test]
    async fn ensure_tls_cert_uses_referenced_secret() {
        let Some(st) = test_state().await else {
            eprintln!("skipping ensure_tls_cert_uses_referenced_secret: DATABASE_URL not set");
            return;
        };
        let dsn = std::env::var("DATABASE_URL").unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (wh, _s) = seed_sr_warehouse(&st, &db).await;

        let mut data = std::collections::BTreeMap::new();
        data.insert("tls.crt".to_string(), "CERTPEM".to_string());
        data.insert("tls.key".to_string(), "KEYPEM".to_string());
        let cert_ref = st
            .secrets
            .create_typed(
                None,
                &format!("tls-{}", Uuid::new_v4().simple()),
                None,
                egret_secrets::secret_type::TLS,
                &data,
            )
            .await
            .expect("create tls secret");

        let engine = st
            .engine_mgmt
            .create_engine(
                &format!("trino-{}", Uuid::new_v4().simple()),
                "trino",
                "egret-engines",
                wh.id,
                &serde_json::json!({ "trinoConfig": {} }),
                Some(cert_ref),
            )
            .await
            .expect("create engine row");

        let (keystore, cert) = ensure_tls_cert(&st, &engine)
            .await
            .expect("ensure tls cert");
        assert_eq!(cert, "CERTPEM");
        assert_eq!(keystore, "CERTPEMKEYPEM");

        // The provided cert is pinned on the row (the connector trusts it).
        let refetched = st
            .engine_mgmt
            .get_engine(engine.id)
            .await
            .expect("get_engine")
            .expect("engine present");
        assert_eq!(refetched.coordinator_cert.as_deref(), Some("CERTPEM"));
        assert_eq!(refetched.tls_cert_secret_ref, Some(cert_ref));
    }

    /// Secure-by-default: `create_starrocks_engine` mints + persists a
    /// `root_secret_id` on the created engine UNCONDITIONALLY (no delegation, no
    /// opt-in).
    #[tokio::test]
    async fn create_starrocks_engine_mints_root_password() {
        let Some(mut st) = test_state().await else {
            eprintln!("skipping create_starrocks_engine_mints_root_password: DATABASE_URL not set");
            return;
        };
        st.engine_provisioner = Some(std::sync::Arc::new(NoopProvisioner));
        let dsn = std::env::var("DATABASE_URL").unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (wh, _secret) = seed_sr_warehouse(&st, &db).await;

        let mut body = sr_update_body(None);
        body.name = format!("sr-{}", Uuid::new_v4().simple());
        let name = body.name.clone();

        let (code, _dto) = create_starrocks_engine(st.clone(), body, wh)
            .await
            .expect("create_starrocks_engine");
        assert_eq!(code, StatusCode::CREATED);

        let created = st
            .engine_mgmt
            .list_engines()
            .await
            .expect("list_engines")
            .into_iter()
            .find(|e| e.name == name)
            .expect("created engine present");
        assert!(
            created.root_secret_id.is_some(),
            "new StarRocks engine must have a minted root_secret_id (secure-by-default)"
        );
    }

    /// `build_starrocks_provision_req` threads the stored root password into
    /// `StarRocksConfig.root_password` when the engine carries a `root_secret_id`,
    /// and leaves it `None` for a legacy engine (no `root_secret_id`).
    #[tokio::test]
    async fn build_starrocks_provision_req_threads_root_password() {
        let Some(st) = test_state().await else {
            eprintln!(
                "skipping build_starrocks_provision_req_threads_root_password: DATABASE_URL not set"
            );
            return;
        };
        let dsn = std::env::var("DATABASE_URL").unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (wh, _secret) = seed_sr_warehouse(&st, &db).await;

        // Legacy engine (created directly, never minted) → root_password None.
        let legacy = st
            .engine_mgmt
            .create_engine(
                &format!("sr-{}", Uuid::new_v4().simple()),
                "starrocks",
                "egret-engines",
                wh.id,
                &serde_json::json!({ "starrocksConfig": {} }),
                None,
            )
            .await
            .expect("create legacy engine row");
        let (legacy_req, _) = build_starrocks_provision_req(&st, &legacy, &wh)
            .await
            .expect("build (legacy)");
        assert!(
            legacy_req
                .starrocks
                .as_ref()
                .expect("sr config")
                .root_password
                .is_none(),
            "legacy engine (no root_secret_id) must stay passwordless"
        );

        // Engine WITH a minted root secret → root_password Some(plaintext).
        let (secret_id, plaintext) = ensure_root_password_secret(&st, &legacy)
            .await
            .expect("mint root password");
        let refetched = st
            .engine_mgmt
            .get_engine(legacy.id)
            .await
            .expect("get_engine")
            .expect("engine present");
        assert_eq!(refetched.root_secret_id, Some(secret_id));
        let (req, _) = build_starrocks_provision_req(&st, &refetched, &wh)
            .await
            .expect("build (passworded)");
        assert_eq!(
            req.starrocks.as_ref().expect("sr config").root_password,
            Some(plaintext)
        );
    }

    /// A minimal StarRocks-shaped `CreateEngineBody` for update tests (only
    /// `kind` + `starrocks` are read by `update_starrocks_engine`; the Trino
    /// wizard fields are ignored).
    fn sr_update_body(starrocks: Option<StarRocksConfigBody>) -> CreateEngineBody {
        CreateEngineBody {
            name: "ignored".into(),
            warehouse_id: Uuid::nil(),
            image_version: None,
            coordinator_replicas: 0,
            worker_replicas: 0,
            cpu_min: String::new(),
            cpu_max: String::new(),
            memory: String::new(),
            trino_config: None,
            authz_service_id: None,
            kind: "starrocks".into(),
            starrocks,
            tls_cert_secret_ref: None,
        }
    }

    /// Seed a stored StarRocks engine (1 FE / 1 CN, no delegation) for the
    /// update-path tests.
    async fn seed_sr_engine(st: &CpState, wh_id: Uuid) -> Engine {
        let spec = serde_json::json!({
            "imageVersion": DEFAULT_SR_FE_IMAGE,
            "starrocksConfig": {
                "feImage": DEFAULT_SR_FE_IMAGE,
                "cnImage": DEFAULT_SR_CN_IMAGE,
                "feReplicas": 1,
                "cnReplicas": 1,
                "feResources": null,
                "cnResources": null,
                "delegation": null,
            },
        });
        st.engine_mgmt
            .create_engine(
                &format!("sr-{}", Uuid::new_v4().simple()),
                "starrocks",
                "egret-engines",
                wh_id,
                &spec,
                None,
            )
            .await
            .expect("create engine row")
    }

    /// A PUT that bumps `feReplicas` + sets `feResources` re-persists those into
    /// the stored spec while preserving the untouched `cnReplicas`/`feImage`.
    /// `test_state` has no k8s provisioner, so the update persists the spec then
    /// fails at the provision step — the DB-visible spec change is what matters.
    #[tokio::test]
    async fn update_starrocks_engine_rescales() {
        let Some(st) = test_state().await else {
            eprintln!("skipping update_starrocks_engine_rescales: DATABASE_URL not set");
            return;
        };
        let dsn = std::env::var("DATABASE_URL").unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (wh, _secret) = seed_sr_warehouse(&st, &db).await;
        let engine = seed_sr_engine(&st, wh.id).await;

        let body = sr_update_body(Some(StarRocksConfigBody {
            fe_replicas: Some(2),
            fe_resources: Some(serde_json::json!({ "memRequest": "2Gi", "memLimit": "4Gi" })),
            ..Default::default()
        }));
        // Ok(Json<EngineDto>) isn't Debug, so map to the error message first.
        let err = update_starrocks_engine(st.clone(), engine.clone(), body)
            .await
            .err()
            .map(|e| e.to_string());
        assert!(
            matches!(&err, Some(m) if m.contains("kubernetes not configured")),
            "expected k8s-not-configured after persist, got {err:?}"
        );

        let reloaded = st
            .engine_mgmt
            .get_engine(engine.id)
            .await
            .expect("reload")
            .expect("present");
        let cfg = &reloaded.spec["starrocksConfig"];
        assert_eq!(cfg["feReplicas"], 2);
        assert_eq!(cfg["cnReplicas"], 1); // untouched ⇒ preserved
        assert_eq!(cfg["feResources"]["memRequest"], "2Gi");
        assert_eq!(cfg["feResources"]["memLimit"], "4Gi");
        assert_eq!(cfg["feImage"], DEFAULT_SR_FE_IMAGE); // untouched ⇒ preserved
    }

    /// An update must never leak storage coords into the stored spec, and the
    /// re-assembled provision request must still derive `storage_path` from the
    /// warehouse (`<bucket>/starrocks/<engine>`), unchanged by the config edit.
    #[tokio::test]
    async fn update_starrocks_engine_preserves_storage() {
        let Some(st) = test_state().await else {
            eprintln!("skipping update_starrocks_engine_preserves_storage: DATABASE_URL not set");
            return;
        };
        let dsn = std::env::var("DATABASE_URL").unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (wh, _secret) = seed_sr_warehouse(&st, &db).await;
        let engine = seed_sr_engine(&st, wh.id).await;

        let body = sr_update_body(Some(StarRocksConfigBody {
            cn_replicas: Some(4),
            ..Default::default()
        }));
        let _ = update_starrocks_engine(st.clone(), engine.clone(), body).await; // fails at prov; spec persisted

        let reloaded = st
            .engine_mgmt
            .get_engine(engine.id)
            .await
            .expect("reload")
            .expect("present");
        // No storage keys leaked into the persisted spec.
        let cfg = &reloaded.spec["starrocksConfig"];
        assert!(cfg.get("storageEndpoint").is_none());
        assert!(cfg.get("storagePath").is_none());
        assert!(cfg.get("storageSecretKey").is_none());
        assert!(cfg.get("storageAccessKey").is_none());

        // The provision req still resolves storage from the warehouse formula.
        let (req, _) = build_starrocks_provision_req(&st, &reloaded, &wh)
            .await
            .expect("build_starrocks_provision_req");
        let sr = req.starrocks.as_ref().expect("starrocks config present");
        assert_eq!(
            sr.storage_path,
            format!("warehouse/starrocks/{}", reloaded.name)
        );
        assert_eq!(sr.cn_replicas, 4);
    }

    /// The generic `build_provision_req` must dispatch a stored StarRocks engine
    /// to the StarRocks assembler (kind StarRocks, `starrocks` populated), not
    /// build a Trino spec.
    #[tokio::test]
    async fn build_provision_req_dispatches_starrocks() {
        let Some(st) = test_state().await else {
            eprintln!("skipping build_provision_req_dispatches_starrocks: DATABASE_URL not set");
            return;
        };
        let dsn = std::env::var("DATABASE_URL").unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (wh, _secret) = seed_sr_warehouse(&st, &db).await;
        let engine = seed_sr_engine(&st, wh.id).await;

        let req = build_provision_req(&st, &engine)
            .await
            .expect("build_provision_req");
        assert_eq!(req.kind, egret_engine_mgmt_api::EngineKind::StarRocks);
        assert!(req.starrocks.is_some());
        assert!(req.catalogs.is_empty());
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

    /// [`starrocks_external_catalog_sql`] (T6) renders a `CREATE EXTERNAL
    /// CATALOG` DDL carrying the REST-catalog uri/warehouse properties + the
    /// OAuth2 client-credentials block (self-refreshing, no static bearer)
    /// StarRocks expects — pure/unit-testable; the live connection + execution
    /// is smoke-only (Task 8).
    #[test]
    fn starrocks_external_catalog_sql_renders_oauth2_rest_properties() {
        let sql = starrocks_external_catalog_sql(
            "demo",
            "http://host.docker.internal:8080",
            "lakehouse",
            "cid:secret",
        );
        assert!(sql.contains("CREATE EXTERNAL CATALOG"), "sql={sql}");
        assert!(sql.contains(r#"iceberg.catalog.type"="rest"#), "sql={sql}");
        assert!(
            sql.contains(r#"iceberg.catalog.uri"="http://host.docker.internal:8080/catalog"#),
            "sql={sql}"
        );
        assert!(
            sql.contains(r#"iceberg.catalog.warehouse"="lakehouse"#),
            "sql={sql}"
        );
        // OAuth2 self-refreshing credentials (mirrors the Trino path), NOT a
        // static bearer — using the Iceberg-standard REST property NAMES
        // (`credential` + `oauth2-server-uri`), verified live on StarRocks 3.3
        // (the `iceberg.catalog.oauth2.*` dot-form is silently ignored).
        assert!(
            sql.contains(r#"iceberg.catalog.credential"="cid:secret"#),
            "sql={sql}"
        );
        assert!(
            sql.contains(
                r#"iceberg.catalog.oauth2-server-uri"="http://host.docker.internal:8080/oauth/token"#
            ),
            "sql={sql}"
        );
        // No static token property remains.
        assert!(!sql.contains("iceberg.catalog.token"), "sql={sql}");
        assert!(sql.contains("`demo`"), "catalog name; sql={sql}");
    }

    /// A warehouse name (free-form, un-validated) containing a `"` must be
    /// escaped so it can't break out of the double-quoted property value and
    /// inject arbitrary DDL that then runs as `root` on the FE. Likewise a
    /// backtick in the catalog identifier must be doubled, not close the
    /// identifier quoting early.
    #[test]
    fn starrocks_external_catalog_sql_escapes_ddl_injection() {
        // Warehouse name tries to close the value and append a DROP.
        let evil_wh = r#"wh" ) ; DROP DATABASE x ; -- "#;
        let sql =
            starrocks_external_catalog_sql("cat`evil", "http://egret:8080", evil_wh, "cid:secret");
        // The raw (unescaped) injection payload must NOT appear verbatim.
        assert!(
            !sql.contains(r#"wh" ) ; DROP DATABASE x"#),
            "unescaped warehouse breakout present; sql={sql}"
        );
        // The `"` inside the warehouse is backslash-escaped in place, so the
        // value stays a single quoted string (the trailing `"` is the property's
        // own closing quote, unescaped).
        assert!(
            sql.contains(r#"iceberg.catalog.warehouse"="wh\" ) ; DROP DATABASE x ; -- ""#),
            "warehouse not escaped; sql={sql}"
        );
        // The backtick in the identifier is doubled, not left to close early.
        assert!(
            sql.contains("`cat``evil`"),
            "identifier not escaped; sql={sql}"
        );
        // Backslash is escaped before the quote so it isn't double-processed.
        let sql2 = starrocks_external_catalog_sql("c", "http://egret:8080", r#"a\b"#, "x:y");
        assert!(
            sql2.contains(r#"warehouse"="a\\b"#),
            "backslash not escaped; sql={sql2}"
        );
    }

    /// [`starrocks_jwt_integration_sql`] (S3-T2) renders the
    /// `authentication_jwt` integration + activates it via the
    /// `authentication_chain` frontend config — mirrors slice-1's manual
    /// `security-integration.sql`, now templated with the engine's own OIDC
    /// coordinates.
    #[test]
    fn starrocks_jwt_integration_sql_renders_integration_and_activation() {
        let stmts = starrocks_jwt_integration_sql(
            "https://kc/realms/egret/protocol/openid-connect/certs",
            "https://kc/realms/egret",
            "egret",
        );
        assert_eq!(stmts.len(), 2);
        assert!(stmts[0].contains("CREATE SECURITY INTEGRATION egret_oidc"));
        assert!(stmts[0].contains(r#""type"="authentication_jwt""#));
        assert!(stmts[0]
            .contains(r#""jwks_url"="https://kc/realms/egret/protocol/openid-connect/certs""#));
        assert!(stmts[0].contains(r#""principal_field"="preferred_username""#));
        assert!(stmts[0].contains(r#""required_issuer"="https://kc/realms/egret""#));
        assert!(stmts[0].contains(r#""required_audience"="egret""#));
        assert_eq!(
            stmts[1],
            r#"ADMIN SET FRONTEND CONFIG ("authentication_chain"="native,egret_oidc")"#
        );
    }

    /// A malicious/free-form issuer/audience/jwks_url must not break out of
    /// the double-quoted property value — same DDL-injection posture as
    /// `starrocks_external_catalog_sql`.
    #[test]
    fn starrocks_jwt_integration_sql_escapes_ddl_injection() {
        let evil = r#"kc" ) ; DROP DATABASE x ; -- "#;
        let stmts = starrocks_jwt_integration_sql("https://kc/jwks", evil, "aud");
        assert!(
            !stmts[0].contains(r#"kc" ) ; DROP DATABASE x"#),
            "unescaped issuer breakout present; stmt={}",
            stmts[0]
        );
        assert!(
            stmts[0].contains(r#"required_issuer"="kc\" ) ; DROP DATABASE x ; -- ""#),
            "issuer not escaped; stmt={}",
            stmts[0]
        );
    }

    /// [`starrocks_create_service_user_sql`] (S3-T2) renders a `CREATE USER`
    /// statement with a single-quoted `IDENTIFIED BY` password; a `'` in
    /// either the username or password must be escaped (doubled), not close
    /// the literal early.
    #[test]
    fn starrocks_create_service_user_sql_renders_and_escapes() {
        let sql = starrocks_create_service_user_sql("egret_delegator", "s3cr3t");
        assert_eq!(
            sql,
            "CREATE USER IF NOT EXISTS 'egret_delegator'@'%' IDENTIFIED BY 's3cr3t'"
        );
        let evil_sql = starrocks_create_service_user_sql("a'; DROP USER root; --", "p'w");
        assert!(
            !evil_sql.contains("a'; DROP USER root"),
            "unescaped username breakout present; sql={evil_sql}"
        );
        assert!(evil_sql.contains("a''; DROP USER root; --"));
        assert!(evil_sql.contains("p''w"));
    }

    /// [`starrocks_impersonation_grant_sql`] (S3-T2) renders `GRANT
    /// IMPERSONATE ON ALL USERS` for the backtick-quoted service user; a
    /// backtick in the identifier must be doubled, not close the identifier
    /// quoting early (this DDL runs as `root`).
    #[test]
    fn starrocks_impersonation_grant_sql_renders_and_escapes() {
        assert_eq!(
            starrocks_impersonation_grant_sql("egret_delegator"),
            "GRANT IMPERSONATE ON ALL USERS TO `egret_delegator`"
        );
        let evil = starrocks_impersonation_grant_sql("a`.evil");
        assert_eq!(evil, "GRANT IMPERSONATE ON ALL USERS TO `a``.evil`");
    }

    /// The FE root bootstrap connection (RP-T4) must present the minted `root`
    /// password when the engine carries one, so a passworded FE (T1-T3) accepts
    /// egret's own post-Ready catalog-wire + delegation-bootstrap DDL. `Some(pw)`
    /// -> `pass()` is that password, `user()` stays `root`.
    #[test]
    fn starrocks_root_conn_opts_presents_password_when_present() {
        let opts: mysql_async::Opts =
            starrocks_root_conn_opts("fe.svc", STARROCKS_FE_MYSQL_PORT, Some("s3cr3t"), None)
                .into();
        assert_eq!(opts.user(), Some("root"));
        assert_eq!(opts.pass(), Some("s3cr3t"));
    }

    /// Legacy engines (no `root_secret_id` -> `root_password` `None`) stay
    /// passwordless: the connection presents no password, exactly as before the
    /// root-password hardening. `user()` still `root`, `pass()` `None`.
    #[test]
    fn starrocks_root_conn_opts_stays_passwordless_when_none() {
        let opts: mysql_async::Opts =
            starrocks_root_conn_opts("fe.svc", STARROCKS_FE_MYSQL_PORT, None, None).into();
        assert_eq!(opts.user(), Some("root"));
        assert_eq!(opts.pass(), None);
    }
}
