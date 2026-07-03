//! Control-plane management for query engines (Phase 1: Trino on Kubernetes).
//!
//! Provisioning a Trino auto-wires it to egret's embedded Iceberg catalog: the
//! route resolves the warehouse + storage endpoint, mints a fresh Catalog Client
//! for the warehouse, and threads those coordinates into an
//! [`EngineProvisionReq`]. The actual k8s work lives behind the
//! [`EngineProvisioner`] trait, implemented in egret-bin over `egret-orchestrator`
//! so this crate never depends on kube (mirrors `WarehouseProvisioner`).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use egret_auth::principal::Auth;
use egret_catalog::service::embedded::{GrantInput, NewClient};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::require_platform_admin;
use crate::error::CpError;
use crate::CpState;

/// Default Trino image (Stackable `productVersion`) when the body omits it.
const DEFAULT_IMAGE_VERSION: &str = "481";
/// Kubernetes namespace engines are provisioned into (Phase 1: fixed).
const DEFAULT_NAMESPACE: &str = "egret-engines";
/// Shared per-warehouse role that engine Catalog Clients are bound to.
const ENGINE_ROLE_NAME: &str = "engine";

/// Resolved provisioning request handed to egret-bin (no k8s types here). The
/// route has already resolved the warehouse name, storage endpoint, and a
/// freshly-minted Catalog Client credential.
pub struct EngineProvisionReq {
    pub name: String,
    pub namespace: String,
    pub image_version: String,
    pub coordinator_replicas: u32,
    pub worker_replicas: u32,
    pub cpu_min: String,
    pub cpu_max: String,
    pub memory: String,
    pub catalog_uri: String,
    pub token_uri: String,
    pub warehouse: String,
    pub s3_endpoint: String,
    pub client_id: String,
    pub client_secret: String,
}

/// One live pod backing an engine.
pub struct EnginePod {
    pub name: String,
    pub phase: String,
    pub ready: bool,
}

/// Live engine status read back from the operator + pods.
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
impl From<egret_catalog::service::Engine> for EngineDto {
    fn from(e: egret_catalog::service::Engine) -> Self {
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

/// Extract the internal Catalog Client row id stashed in an engine's `spec`.
fn client_row_id(spec: &serde_json::Value) -> Option<Uuid> {
    spec.get("clientRowId")
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

    // 4. Build the provisioning request.
    let image_version = b
        .image_version
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_IMAGE_VERSION.into());
    let namespace = DEFAULT_NAMESPACE.to_string();
    let req = EngineProvisionReq {
        name: b.name.clone(),
        namespace: namespace.clone(),
        image_version: image_version.clone(),
        coordinator_replicas: b.coordinator_replicas,
        worker_replicas: b.worker_replicas,
        cpu_min: b.cpu_min.clone(),
        cpu_max: b.cpu_max.clone(),
        memory: b.memory.clone(),
        catalog_uri: format!("{}/catalog", st.engine_egret_url),
        token_uri: format!("{}/oauth/token", st.engine_egret_url),
        warehouse: wh.name.clone(),
        s3_endpoint,
        client_id: client.client_id.clone(),
        client_secret: client.client_secret.clone(),
    };

    // 5. Record the intent row (status `provisioning`).
    let spec = serde_json::json!({
        "coordinatorReplicas": b.coordinator_replicas,
        "workerReplicas": b.worker_replicas,
        "cpuMin": b.cpu_min,
        "cpuMax": b.cpu_max,
        "memory": b.memory,
        "imageVersion": image_version,
        "clientRowId": client.id,
    });
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

    // 6. Provision on Kubernetes; roll back the row + client on any failure.
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
    let wh = st
        .storage
        .get_warehouse(engine.warehouse_id)
        .await?
        .ok_or(CpError::NotFound)?;
    let sc = st
        .storage
        .get_storage_conn(wh.storage_conn_id)
        .await?
        .ok_or_else(|| CpError::BadRequest("unknown storage connection".into()))?;
    let s3_endpoint = sc.endpoint.unwrap_or_default();

    // Re-mint a fresh Catalog Client so the re-applied Secret has valid creds
    // (the plaintext secret is only ever shown once).
    let role_id = ensure_engine_role(&st, wh.id).await?;
    let client = st
        .storage
        .create_client(&NewClient {
            name: format!("engine-{}", engine.name),
            consumer_type: "engine".into(),
            workspace_id: wh.workspace_id,
            warehouse_id: wh.id,
            catalog_role_id: role_id,
        })
        .await?;

    let image_version = b
        .image_version
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_IMAGE_VERSION.into());
    let req = EngineProvisionReq {
        name: engine.name.clone(),
        namespace: engine.namespace.clone(),
        image_version: image_version.clone(),
        coordinator_replicas: b.coordinator_replicas,
        worker_replicas: b.worker_replicas,
        cpu_min: b.cpu_min.clone(),
        cpu_max: b.cpu_max.clone(),
        memory: b.memory.clone(),
        catalog_uri: format!("{}/catalog", st.engine_egret_url),
        token_uri: format!("{}/oauth/token", st.engine_egret_url),
        warehouse: wh.name.clone(),
        s3_endpoint,
        client_id: client.client_id.clone(),
        client_secret: client.client_secret.clone(),
    };

    let Some(prov) = st.engine_provisioner.as_ref() else {
        let _ = st.storage.revoke_client(client.id).await;
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };
    if let Err(msg) = prov.update(&req).await {
        let _ = st.storage.revoke_client(client.id).await;
        return Err(CpError::BadRequest(format!("engine update failed: {msg}")));
    }

    // Success: retire the previous client, persist the new spec.
    if let Some(old) = client_row_id(&engine.spec) {
        let _ = st.storage.revoke_client(old).await;
    }
    let spec = serde_json::json!({
        "coordinatorReplicas": b.coordinator_replicas,
        "workerReplicas": b.worker_replicas,
        "cpuMin": b.cpu_min,
        "cpuMax": b.cpu_max,
        "memory": b.memory,
        "imageVersion": image_version,
        "clientRowId": client.id,
    });
    st.storage.update_engine(id, &spec, &engine.status).await?;
    let engine = st.storage.get_engine(id).await?.ok_or(CpError::NotFound)?;
    Ok(Json(engine.into()))
}

async fn delete_engine(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, CpError> {
    require_platform_admin(&p)?;
    let engine = st.storage.get_engine(id).await?.ok_or(CpError::NotFound)?;
    if let Some(prov) = st.engine_provisioner.as_ref() {
        prov.delete(&engine.namespace, &engine.name)
            .await
            .map_err(|e| CpError::BadRequest(format!("engine delete failed: {e}")))?;
    }
    st.storage.delete_engine(id).await?;
    // Best-effort: retire the auto-wired Catalog Client.
    if let Some(cid) = client_row_id(&engine.spec) {
        let _ = st.storage.revoke_client(cid).await;
    }
    Ok(StatusCode::NO_CONTENT)
}
