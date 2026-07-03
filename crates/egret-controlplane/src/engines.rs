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

/// Fuller Trino config surface, kube-free (mirrors the orchestrator `TrinoConfig`).
#[derive(Clone, Default)]
pub struct TrinoConfigReq {
    pub jvm_heap: Option<String>,
    pub query_max_memory: Option<String>,
    pub query_max_memory_per_node: Option<String>,
    pub config_properties: BTreeMap<String, String>,
    pub env: BTreeMap<String, String>,
}

/// Resolved provisioning request handed to egret-bin (no k8s types here). The
/// route has already resolved every catalog into flat properties + env and the
/// config surface into [`TrinoConfigReq`].
pub struct EngineProvisionReq {
    pub name: String,
    pub namespace: String,
    pub image_version: String,
    pub coordinator_replicas: u32,
    pub worker_replicas: u32,
    pub cpu_min: String,
    pub cpu_max: String,
    pub memory: String,
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
    /// Optional fuller config surface, stored under `spec["trinoConfig"]`. Wire
    /// shape: `{ jvmHeap, queryMaxMemory, queryMaxMemoryPerNode, configProperties,
    /// env }`.
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
    let cfg = engine
        .spec
        .get("trinoConfig")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    Ok(EngineProvisionReq {
        name: engine.name.clone(),
        namespace: engine.namespace.clone(),
        image_version: engine.spec["imageVersion"]
            .as_str()
            .unwrap_or(DEFAULT_IMAGE_VERSION)
            .into(),
        coordinator_replicas: engine.spec["coordinatorReplicas"].as_u64().unwrap_or(1) as u32,
        worker_replicas: engine.spec["workerReplicas"].as_u64().unwrap_or(1) as u32,
        cpu_min: engine.spec["cpuMin"].as_str().unwrap_or("1").into(),
        cpu_max: engine.spec["cpuMax"].as_str().unwrap_or("2").into(),
        memory: engine.spec["memory"].as_str().unwrap_or("4Gi").into(),
        catalogs,
        config: trino_config_from_json(&cfg),
    })
}

fn trino_config_from_json(v: &serde_json::Value) -> TrinoConfigReq {
    let map = |k: &str| -> BTreeMap<String, String> {
        v[k].as_object()
            .map(|o| {
                o.iter()
                    .filter_map(|(k, val)| val.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default()
    };
    TrinoConfigReq {
        jvm_heap: v["jvmHeap"].as_str().map(Into::into),
        query_max_memory: v["queryMaxMemory"].as_str().map(Into::into),
        query_max_memory_per_node: v["queryMaxMemoryPerNode"].as_str().map(Into::into),
        config_properties: map("configProperties"),
        env: map("env"),
    }
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
    if let Some(tc) = &b.trino_config {
        spec["trinoConfig"] = tc.clone();
    }
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
