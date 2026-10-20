//! Control-plane management for query engines (Trino on Kubernetes).
//!
//! Provisioning a Trino auto-wires it to egret's embedded Iceberg catalog: the
//! route attaches the warehouse's registry catalog with a freshly minted Catalog
//! Client (its credential in a named secret) as the engine's PRIMARY catalog. A
//! cluster can then attach further registry catalogs (SP3).
//!
//! Catalogs render from registry attachments (`crate::catalog_apply` turns each
//! into a flat `{name, properties, env}` [`CatalogReq`]), and
//! [`build_provision_req`] assembles the full [`EngineProvisionReq`]. The kube-free provisioning ops (provision/status/
//! update/delete/restart) go through the `EngineProvisioningPort`, implemented
//! in egret-bin over `egret-provisioner`. The kube-shaped ops
//! (exec/pod-logs/coordinator-request) go straight through
//! `egret_provisioner::KubeClient`, held on `EngineMgmtState::kube`: `exec`
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

use crate::dependents::{
    DeleteEngineOutcome, EngineConflictBody, EngineDependentDto, EngineDependentKind,
};
use crate::Engine;
use crate::EngineHttpError as CpError;
use crate::EngineMgmtState as CpState;
use egret_catalog_registry::{AttachedCatalog, WarehouseIdentity};
use egret_perm::DEFAULT_ENGINE_IMAGES;

/// Kubernetes namespace engines are provisioned into (fixed for now).
const DEFAULT_NAMESPACE: &str = "egret-engines";
/// Shared per-warehouse role that engine Catalog Clients are bound to.
const ENGINE_ROLE_NAME: &str = "engine";
/// StarRocks FE MySQL-protocol (query) port — `query_port` in `fe.conf`, and
/// what [`crate::store::Engine::coordinator_port`] answers for a StarRocks
/// engine. Since SP3b Task 5 nothing in production reads it here: every
/// statement's port comes from the engine row through
/// `starrocks_ddl::fe_target`. It is kept for the connection-opts tests below,
/// which assert the port the FE really listens on.
#[cfg(test)]
const STARROCKS_FE_MYSQL_PORT: u16 = 9030;

// Canonical engine-spec DTOs now live in the dependency-free `egret-engine-mgmt-api`
// contract crate (shared with `egret-provisioner`); aliased here to the old
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
            "/engines/modelled-ranger-plugins",
            get(list_modelled_ranger_plugins),
        )
        // Register an engine egret did not provision (SP4d-4 Part C).
        .merge(crate::external::routes())
        .route(
            "/engines/{id}",
            get(get_engine).put(update_engine).delete(delete_engine),
        )
        // `POST` here is the registry's attach (`crate::catalogs::attach_catalog`);
        // the legacy `GET`/`PUT`/`DELETE` siblings and the sibling `/catalogs`
        // (list+add) route were removed (SP3c-2 Task 4). `/catalogs/{cid}/test`
        // (below) was ALSO one of the six but was kept — see `test_catalog`'s
        // doc comment: `EngineDetail.tsx`'s per-row Test button is a live
        // caller with no registry-route equivalent.
        .route(
            "/engines/{id}/catalogs/{cid}",
            post(crate::catalogs::attach_catalog),
        )
        .route("/engines/{id}/catalogs/{cid}/test", post(test_catalog))
        .route("/engines/{id}/pods/{pod}/logs", get(pod_logs))
        .route("/engines/{id}/pods/{pod}/logs/follow", get(pod_logs_follow))
        .route("/engines/{id}/pods/{pod}/exec", get(pod_exec))
        .route("/engines/{id}/restart", post(restart_engine))
        .route("/engines/{id}/rotate-secrets", post(rotate_engine_secrets))
        .route(
            "/engines/{id}/rotate-ranger-credential",
            post(rotate_ranger_credential),
        )
        .route(
            "/engines/{id}/retire-previous-ranger-credential",
            post(retire_previous_ranger_credential),
        )
        // Engine↔workspace assignment (P1): list + set-full-assignment-set.
        .route(
            "/engines/{id}/workspaces",
            get(list_engine_workspaces).put(set_engine_workspaces),
        )
        // Native Trino monitor (query list / detail / kill / cluster).
        .merge(crate::monitor::routes())
        // Registry catalogs (SP3): platform CRUD + impact + re-apply.
        .merge(crate::catalogs::routes())
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

/// One catalog to attach when the engine is created.
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NewEngineCatalog {
    catalog_id: Uuid,
    /// Exactly one catalog in the list carries this, and it must be an egret
    /// warehouse catalog: the engine's Ranger role and (for StarRocks) its
    /// builtin storage are derived from that warehouse, and since SP3c-2 they
    /// are read back through this attachment rather than off the engine row.
    /// A primary naming a catalog with no warehouse is refused, with the
    /// reason, before anything is created.
    #[serde(default)]
    primary: bool,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct CreateEngineBody {
    name: String,
    /// Superseded by `catalogs` and dropped in SP3c-2. A body that sends this
    /// and no `catalogs` behaves exactly as it did before; a body that sends
    /// BOTH is refused rather than having one of them quietly ignored.
    #[serde(default)]
    warehouse_id: Option<Uuid>,
    /// Registry catalogs to attach at create. Exactly one must be `primary`,
    /// and every id must exist — an unknown or repeated id refuses the create
    /// before anything is provisioned. Mutually exclusive with `warehouseId`.
    /// CREATE ONLY: `PUT /engines/{id}` refuses a body that carries it.
    #[serde(default)]
    catalogs: Vec<NewEngineCatalog>,
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
    /// provisioner. egret-managed keys ([`egret_engine_mgmt_api::SR_PROTECTED_FE_KEYS`])
    /// are rejected up front by [`validate_sr_fe_config`]. Omitted ⇒ preserved on update.
    fe_config: Option<std::collections::BTreeMap<String, String>>,
    /// CN (`cn.conf`) override keys → rendered into a ConfigMap by the provisioner.
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
    /// StorageClass backing the FE metadata PVC (`/opt/starrocks/fe/meta`).
    /// Omitted/blank ⇒ the cluster's DEFAULT StorageClass. Omitted ⇒ preserved
    /// on update.
    fe_meta_storage_class: Option<String>,
    /// Size of the FE metadata PVC, as a k8s binary quantity (`"20Gi"`).
    /// Omitted ⇒ preserved on update, then
    /// `egret_engine_mgmt_api::DEFAULT_FE_META_STORAGE_SIZE`.
    fe_meta_storage_size: Option<String>,
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
pub(crate) struct EngineDto {
    id: Uuid,
    name: String,
    #[serde(rename = "type")]
    r#type: String,
    namespace: String,
    /// For an external engine this is the row's placeholder `running`, which
    /// means nothing: read `origin` instead (egret does not monitor it).
    status: String,
    spec: EngineSpecDto,
    /// `provisioned` (egret runs the cluster) or `external` (egret holds a row
    /// for a cluster it never provisions, updates, restarts, scales or deletes).
    origin: String,
    /// External only: the host the engine answers on.
    #[serde(skip_serializing_if = "Option::is_none")]
    host: Option<String>,
    /// External only: the port the engine answers on.
    #[serde(skip_serializing_if = "Option::is_none")]
    port: Option<i32>,
    /// Provisioned only: the full image ref the engine's stored spec runs (for
    /// StarRocks the FE image; for Trino the `trinoConfig.image` override when
    /// set, else the bare `imageVersion` expanded to `trinodb/trino:<version>`).
    /// Absent when the spec pins none.
    #[serde(skip_serializing_if = "Option::is_none")]
    image: Option<String>,
    /// The Ranger plugin version egret reads this engine at. External: the
    /// version the operator declared. Provisioned: the version its exact image
    /// ships (`egret_perm::plugin_version_for_image`); absent when the image is
    /// not one egret has measured.
    #[serde(skip_serializing_if = "Option::is_none")]
    ranger_plugin_version: Option<String>,
    /// Whether egret's Ranger evaluator has been read against this engine's
    /// (type, plugin version) PAIR: only the pairs in
    /// `egret_perm::MODELLED_RANGER_PLUGINS` are. When `false`, a migration to the profile bound to this
    /// engine is refused (never evaluated at a guessed version) and `explain`
    /// answers that it cannot. Enforcement is unchanged.
    ranger_plugin_version_modelled: Option<bool>,
    /// Why the engine is not modelled, and how to fix it (for a provisioned
    /// engine: pin its image). Absent when modelled.
    #[serde(skip_serializing_if = "Option::is_none")]
    ranger_plugin_version_note: Option<String>,
}

impl From<Engine> for EngineDto {
    fn from(e: Engine) -> Self {
        let image = if crate::external::is_external(&e) {
            None
        } else {
            egret_perm::stored_engine_image(&e.r#type, &e.spec)
        };
        // DG #3: the one implementation, shared with the Governance Overview.
        let egret_perm::PluginReport {
            version,
            modelled,
            note,
        } = egret_perm::ranger_plugin_report(
            &e.r#type,
            &e.origin,
            e.ranger_plugin_version.as_deref(),
            &e.spec,
        );
        let spec = serde_json::from_value(e.spec).unwrap_or_default();
        EngineDto {
            id: e.id,
            name: e.name,
            r#type: e.r#type,
            namespace: e.namespace,
            status: e.status,
            spec,
            origin: e.origin,
            host: e.external_host,
            port: e.external_port,
            image,
            ranger_plugin_version: version,
            ranger_plugin_version_modelled: Some(modelled),
            ranger_plugin_version_note: note,
        }
    }
}

/// One modelled (engine type, Ranger plugin version) pair.
#[derive(Serialize, ToSchema)]
pub(crate) struct ModelledRangerPluginDto {
    #[serde(rename = "type")]
    r#type: String,
    version: String,
}

#[utoipa::path(
    get,
    path = "/api/control-plane/engines/modelled-ranger-plugins",
    tag = "control-plane-engines",
    responses(
        (status = 200, description = "The (engine type, Ranger plugin version) pairs egret's evaluator is verified against, in declaration order: what an external engine may be registered at and still be modelled", body = Vec<ModelledRangerPluginDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:read permission", body = ErrorBody),
    ),
)]
async fn list_modelled_ranger_plugins(
    State(st): State<CpState>,
    Auth(p): Auth,
) -> Result<Json<Vec<ModelledRangerPluginDto>>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:read").await?;
    Ok(Json(
        egret_perm::MODELLED_RANGER_PLUGINS
            .iter()
            .map(|(t, v)| ModelledRangerPluginDto {
                r#type: (*t).into(),
                version: (*v).into(),
            })
            .collect(),
    ))
}

/// What `POST /engines` returns: the engine, plus whichever of the body's
/// `catalogs` the engine came up WITHOUT.
///
/// Its own type rather than a field on [`EngineDto`] (review F2): `GET`, the
/// list and `PUT` all return `EngineDto` and can never have anything to say
/// here, and a schema that advertises a field an endpoint cannot send is a
/// promise to its consumers that only create keeps. The engine's own fields are
/// flattened, so the create response is byte-identical to `EngineDto` whenever
/// `catalogAttachErrors` is empty — which is every create that does what it was
/// asked.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateEngineResponse {
    #[serde(flatten)]
    engine: EngineDto,
    /// Catalogs from the body's `catalogs` that could not be attached. Empty on
    /// success. The engine EXISTS and is provisioned; these catalogs simply are
    /// not on it, and each can be retried with
    /// `POST /api/control-plane/engines/{id}/catalogs/{cid}`. The create is not
    /// rolled back for them — the cluster is already up by the time they are
    /// attached, and the alternative (the wizard's own attach loop) reports a
    /// per-catalog failure exactly the same way. Every id in the body was
    /// resolved BEFORE provisioning, so nothing here is a mistyped id.
    catalog_attach_errors: Vec<CatalogAttachErrorDto>,
}

/// One catalog from a create body's `catalogs` that the engine came up without.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogAttachErrorDto {
    catalog_id: Uuid,
    /// Why the attach failed, in the words the attach route would have used.
    message: String,
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

/// Knobs for one spec assembly.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct BuildOpts {
    /// Render WITHOUT this registry catalog: detach applies first and deletes the
    /// attachment only after Kubernetes dropped the catalog.
    pub(crate) exclude_catalog: Option<Uuid>,
    /// Render as if this attachment existed: attach applies first and writes the
    /// attachment only after Kubernetes accepted the catalog.
    pub(crate) pending_attachment: Option<PendingAttachment>,
    /// Mint new internal secrets even when the engine already has some.
    pub(crate) rotate_internal_secrets: bool,
    /// Rotate the engine's Ranger plugin credential (A10) instead of reusing
    /// it. Refused with 409 while the previous one is still pending, before
    /// anything is applied.
    pub(crate) rotate_ranger_credential: bool,
}

/// An attachment rendered before it is written (see [`BuildOpts`]).
#[derive(Clone, Copy, Debug)]
pub(crate) struct PendingAttachment {
    pub(crate) catalog_id: Uuid,
    pub(crate) catalog_client_id: Option<Uuid>,
    pub(crate) credential_secret_id: Option<Uuid>,
}

/// An assembled provisioning request plus the internal secrets it rendered
/// (Trino only). Hand it to [`apply_built`], which settles those secrets.
pub(crate) struct BuiltReq {
    req: EngineProvisionReq,
    internal: Option<EngineInternalSecrets>,
    /// Database time just before the catalogs were read (Trino only). A
    /// successful apply marks applied only attachments whose catalog has not
    /// changed since.
    catalogs_read_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Build the full provisioning request from the engine + its stored catalogs + config.
async fn build_provision_req(st: &CpState, engine: &Engine) -> Result<BuiltReq, CpError> {
    build_provision_req_with(st, engine, BuildOpts::default()).await
}

/// The warehouse an already-created engine leads with — the warehouse of its
/// PRIMARY attachment's catalog. There is no `engines.warehouse_id` column
/// any more (SP3c-2 Task 5 stopped reading it; Task 6 dropped it).
///
/// Two facts come off it and both are cluster state rather than a display
/// value: the StarRocks builtin storage volume's S3 coordinates, and (via
/// `ensure_engine_role`) the Ranger role an engine's Catalog Clients are bound
/// to. So a missing primary REFUSES here. Defaulting to "the first attached
/// catalog that happens to have a warehouse" would point a StarRocks FE's
/// storage volume at a bucket the engine was never provisioned against, and
/// the failure would surface as data written to the wrong place rather than as
/// an error — the one outcome an operator cannot undo.
///
/// `POST /engines` refuses a body whose primary names a catalog with no
/// warehouse ([`resolve_primary_catalog`]) and `detach_from_engine` refuses to
/// remove a primary, so an engine created by egret always has one. Migration
/// 0150's partial unique index bounds it from ABOVE only (at most one), which
/// is why the lower bound is checked here and not assumed.
async fn primary_warehouse(
    st: &CpState,
    engine: &Engine,
) -> Result<egret_catalog_api::Warehouse, CpError> {
    let wh_id = st
        .engine_mgmt
        .registry()
        .primary_warehouse_id(engine.id)
        .await?
        .ok_or_else(|| {
            CpError::BadRequest(format!(
                "engine {} has no primary catalog with a warehouse: an engine's storage and \
                 Ranger role are derived from the warehouse of its primary attachment. Attach \
                 an egret warehouse catalog to it and mark that attachment primary",
                engine.name
            ))
        })?;
    st.catalog
        .get_warehouse(wh_id)
        .await?
        .ok_or_else(|| CpError::BadRequest("unknown warehouse".into()))
}

/// [`build_provision_req`] with explicit [`BuildOpts`].
pub(crate) async fn build_provision_req_with(
    st: &CpState,
    engine: &Engine,
    opts: BuildOpts,
) -> Result<BuiltReq, CpError> {
    // StarRocks engines assemble an entirely different `EngineSpec` (FE/CN roles
    // + builtin storage volume, no Trino catalogs/auth). Dispatch to the shared
    // StarRocks assembler so ANY generic caller of `build_provision_req` is
    // correct for a StarRocks engine. (`update_starrocks_engine` calls
    // `build_starrocks_provision_req` directly instead, so it can also capture
    // the delegation bootstrap a later task will consume.)
    if engine.r#type == "starrocks" {
        let wh = primary_warehouse(st, engine).await?;
        let (req, _bootstrap) =
            build_starrocks_provision_req_with(st, engine, &wh, opts.rotate_ranger_credential)
                .await?;
        return Ok(BuiltReq {
            req,
            internal: None,
            catalogs_read_at: None,
        });
    }
    // Resolved before anything is minted, so a spec with no image leaks nothing.
    let image_version = stored_image(engine, &["imageVersion"])?;
    // Catalogs come from the registry (SP3).
    let registry = st.engine_mgmt.registry();
    let catalogs_read_at = registry.db_now().await?;
    let mut attached = registry.list_engine_attachments(engine.id).await?;
    attached.retain(|a| Some(a.catalog.id) != opts.exclude_catalog);
    if let Some(p) = opts.pending_attachment {
        let catalog = registry
            .get_catalog(p.catalog_id)
            .await?
            .ok_or(CpError::NotFound)?;
        attached.push(AttachedCatalog {
            catalog,
            attachment: egret_catalog_registry::AttachmentRow::pending(
                engine.id,
                p.catalog_id,
                p.catalog_client_id,
                p.credential_secret_id,
            ),
        });
    }
    let mut catalogs = Vec::with_capacity(attached.len());
    for (i, a) in attached.iter().enumerate() {
        catalogs.push(crate::catalog_apply::trino_catalog_req(st, a, &format!("C{i}")).await?);
    }
    let mut config = build_trino_config(&engine.spec);
    // Resolve the fault-tolerant exchange storage connection (if any) into S3
    // coords + creds, mirroring the egret-warehouse arm's storage resolution.
    config.fault_tolerant =
        resolve_fault_tolerant(st, &engine.spec["trinoConfig"], &mut config.env).await?;
    // Resolve the RAW auth block (password secrets → bcrypt hashes, JWKS default),
    // mirroring the fault-tolerant storage resolution. Ingress is parsed inline.
    config.auth = resolve_auth(st, &engine.spec["trinoConfig"]).await?;
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
    // Image-pull from a typed secret: resolve the referenced
    // kubernetes.io/dockerconfigjson secret, stash its `.dockerconfigjson`
    // payload for the provisioner to project into `{name}-pull`, and append
    // that Secret name to `imagePullSecrets`.
    if let Some(ref_id) = config.image_pull_secret_ref {
        let json = resolve_dockerconfigjson_secret(st, ref_id).await?;
        config.image_pull_secret_json = Some(json);
        let pull = format!("{}-pull", engine.name);
        if !config.image_pull_secrets.contains(&pull) {
            config.image_pull_secrets.push(pull);
        }
    }
    // Late-bind (R1): an engine created while Ranger was disabled keeps
    // `authz_service_id = NULL` forever — `set_engine_authz_service` has one
    // caller, reachable only from create — so once an operator turns Ranger
    // back on, every later apply would otherwise keep re-rendering explicit
    // allow-all with no operator-accessible remedy. Reuses the SAME
    // plan/bind a create uses (create / adopt-orphan / 409-if-bound-
    // elsewhere). Any refusal - an overlap the bind would create or any other
    // failure - fails the apply with its cause; the engine is never left unbound.
    let mut authz_service_id = engine.authz_service_id;
    if st.ranger_enabled && authz_service_id.is_none() {
        match plan_trino_profile(st, &engine.name, None).await {
            Ok(plan) => {
                let mut undo = CreateUndo::default();
                match bind_trino_profile(st, engine.id, &engine.name, &plan, &mut undo).await {
                    Ok(()) => {
                        authz_service_id = match plan {
                            ProfilePlan::Existing(id) => Some(id),
                            ProfilePlan::Create => undo.created_profile_id,
                            ProfilePlan::Unbound => None,
                        };
                        // The profile is newly reachable from this engine's
                        // catalogs, which are already attached on this path —
                        // compile the `perm` model into it now rather than
                        // leaving it stale until someone calls the manual
                        // trigger (SP4a Task 9 review).
                        crate::profile_compiler::recompile_bound_profile(
                            st,
                            engine.id,
                            &engine.name,
                        )
                        .await;
                    }
                    // A bind that would create an order-ambiguous pair fails the
                    // apply with the structured 409, as create and PUT do: the
                    // guarded bind rolled back, and an engine must not come out
                    // of an apply unbound (allow-all) while Ranger is enabled.
                    Err(e @ CpError::RuleOverlap(_)) => return Err(e),
                    // Any OTHER failure fails the apply, naming the cause: an
                    // engine must not come out of an apply unbound (allow-all)
                    // while Ranger is enabled. Nothing is half-applied - this
                    // runs before the cluster is touched, the bind is one
                    // transaction, and a profile the bind created is adopted by
                    // the retry's plan (`Existing`) - so the operator can retry.
                    Err(e) => {
                        tracing::error!(engine = %engine.name, error = %e,
                            "late Ranger bind failed; the apply is refused");
                        return Err(e);
                    }
                }
            }
            Err(e) => {
                tracing::error!(engine = %engine.name, error = %e,
                    "late Ranger bind plan failed; the apply is refused");
                return Err(e);
            }
        }
    }
    // Impersonate-mode delegation: when auth is on, provision the `egret_delegator`
    // service user (generate + store its password once, inject its bcrypt into the
    // auth block so it lands in `password.db`) and seed the delegator's impersonate
    // Ranger policy onto the bound Trino service (the per-user baseline is seeded
    // below, for both modes). Guarded on auth being
    // enabled — without a `password.db` the delegator can't basic-auth, so
    // impersonate delegation is meaningless (and `inject_delegator_user` no-ops).
    let ranger_service_name = resolve_ranger_service_name(st, authz_service_id).await?;
    if config.auth.is_some() && engine.is_impersonate_delegation() {
        let (_secret_id, pw) = ensure_delegator_secret(st, engine).await?;
        inject_delegator_user(&mut config.auth, &pw)?;
        if let Some(service_name) = ranger_service_name.as_deref() {
            seed_delegator_policies(st, service_name).await?;
        }
    }
    // Ranger auto-admin (sub-project A): grant each platform admin all-access on
    // the bound Ranger service so a governed engine is immediately queryable with
    // no manual policy seeding. Fires for impersonate AND jwt. Idempotent (upsert).
    if let Some(service_name) = ranger_service_name.as_deref() {
        if engine.supports_delegation() {
            if let Some(svc) = st
                .authz
                .get_service_opt(service_name)
                .await
                .map_err(|e| CpError::BadRequest(format!("resolve ranger service: {e}")))?
            {
                // This hook only runs on the Trino provision path (the StarRocks
                // path returns early into `build_starrocks_provision_req`), so the
                // service type is always "trino".
                let admins = admin_usernames(st).await?;
                st.authz
                    .seed_admin_all_access(svc.id, service_name, "trino", &admins)
                    .await
                    .map_err(|e| CpError::BadRequest(format!("seed admin policy: {e}")))?;
                recompile_after_infra_seed(st, svc.id).await;
            }
            // The per-user baseline, for impersonate AND jwt: every delegated
            // session is checked for it (DG Task 7).
            seed_user_baseline(st, service_name).await?;
        }
    }
    // When the engine is bound to a Ranger profile — named by
    // `resolve_ranger_service_name` from the BOUND profile row, not assumed to
    // match the engine — template the apache-ranger access-control config so
    // the bundled plugin polls egret's Ranger-Admin download API (Access P3.2),
    // authenticating with the engine's own credential (A9). Settled here, after
    // every other fallible step and before the internal secrets are minted, so
    // a refused rotation (409) leaves nothing to discard.
    let ranger = ranger_binding(
        st,
        engine,
        ranger_service_name.clone(),
        opts.rotate_ranger_credential,
    )
    .await?;
    // Internal secrets come LAST: minting writes new secret rows, so every
    // fallible step above has already passed, and a failure below cleans up
    // after itself. `apply_built` records or discards them.
    let internal = resolve_internal_secrets(st, engine, opts.rotate_internal_secrets).await?;
    // The native monitor signs in as `egret-monitor`. Skipped for jwt-delegation
    // engines: their coordinator is JWT-only (no PASSWORD authenticator), so a
    // basic-auth monitor user can't authenticate — the monitor degrades there.
    if !engine.is_jwt_delegation() {
        if let Err(e) = inject_monitor_user(&mut config.auth, &internal.monitor) {
            discard_internal_secrets(st, &internal).await;
            return Err(e);
        }
    }
    // Trino requires a coordinator<->worker shared secret whenever auth is
    // enabled; both roles read it from the env Secret.
    if config.auth.is_some() {
        config
            .env
            .insert("TRINO_INTERNAL_SECRET".into(), internal.internal.clone());
    }
    Ok(BuiltReq {
        req: EngineProvisionReq {
            name: engine.name.clone(),
            namespace: engine.namespace.clone(),
            image_version,
            catalogs,
            config,
            ranger,
            kind: egret_engine_mgmt_api::EngineKind::Trino,
            starrocks: None,
        },
        internal: Some(internal),
        catalogs_read_at: Some(catalogs_read_at),
    })
}

/// Which provisioning call an [`apply_built`] makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ApplyOp {
    Provision,
    Update,
}

/// Apply an assembled request and settle the internal secrets it rendered:
/// recorded on the row on BOTH the success and the failure path. `engine` must
/// be the row the request was built from.
///
/// This does NOT discard on a failed apply. `TrinoProvisioner::apply_all`
/// writes the env Secret and the `password.db` ConfigMap BEFORE the
/// Deployments and does not roll those back on a later failure (e.g. a bad
/// Deployment spec, or the API server timing out on the last object) — so a
/// failed `provision`/`update` call may still have left the CLUSTER holding
/// the fresh pair. Discarding it in that case would delete a secret the
/// running pods already carry, and the next pod roll would then 401 the
/// monitor with no successful apply left to recover it. Committing
/// unconditionally means the row always names the pair the apply ATTEMPTED to
/// hand the cluster; the failure mode this trades for is the opposite one — an
/// apply that failed before touching the Secret at all leaves the row naming a
/// value no pod has yet, so the monitor may 401 until the NEXT successful
/// apply, which resolves the already-committed pair (`resolve_internal_secrets`
/// reuses it) and converges. A failure to record is logged, not returned — the
/// cluster attempt already happened either way.
///
/// Standing rule: egret never deletes a secret value the cluster may still
/// hold. `apply_all` may write the fresh pair to the live env Secret before
/// failing on a LATER step (a bad Deployment spec, an admission webhook, a
/// timeout) — but it can just as easily fail BEFORE touching the Secret at
/// all (an RBAC denial or a timeout on `trino_env_secret` itself), in which
/// case the cluster never saw the fresh pair and still runs on the pair this
/// call is about to supersede. `apply_built` cannot tell those two failures
/// apart from here, so on ANY `Err` it keeps the superseded pair (passes
/// [`Supersede::Keep`]) rather than risk deleting a value the running pods
/// still carry. The row still names the new pair either way (see above) — a
/// failed apply therefore leaves EVERY pair still on record resolvable
/// (including ones left behind by an earlier failed apply on this same
/// engine), and only the next successful apply deletes anything — and even
/// then ONLY the pair its own compare-and-set displaced
/// (`commit_internal_secrets`'s doc comment explains why a wider,
/// prefix-based sweep is unsafe: a concurrent apply's already-minted, not-yet-
/// committed pair could be caught in it). A run of failed rotations can
/// therefore leave orphaned generations behind indefinitely; only engine
/// delete (Task 15) is a safe place to sweep them, since by then no apply can
/// still be in flight for this engine.
/// Failure from [`apply_built`]: either the row was gone or `deleting` when
/// re-checked, before the provisioner was touched, or the provisioner itself
/// rejected the apply. Kept distinct so every caller can turn the former into
/// `CpError::Conflict` (409, retry) instead of the apply-failure shape it uses
/// for the latter.
#[derive(Debug)]
pub(crate) enum ApplyBuiltError {
    /// The row is gone, or is `deleting` (a concurrent DELETE), or re-reading
    /// it to check failed outright. Either way `apply_built` cannot confirm
    /// it is still safe to call the provisioner, so it never does.
    Conflict,
    Failed(String),
}

/// Re-checked immediately before the provisioner call, because every caller
/// does real work between reading `engine` and this point — long enough for
/// a concurrent DELETE to mark the row `deleting` and tear the cluster down.
/// `prov.provision`/`prov.update` is server-side apply: called on a deleted
/// engine it would RE-CREATE every object DELETE just removed, including
/// minting a fresh internal-secret pair the row can never reference again
/// (SP1 Part B final review, C1). Refusing here is the one place all six
/// apply routes share, so it is the one place this guard needs to live.
///
/// The guard refuses ONLY on `deleting` or a missing row — NOT on any other
/// status change. An earlier version demanded the row still be at the exact
/// status the caller had observed, which 409'd a legitimate apply landing
/// between `GET /engines/{id}` recording `provisioning → running` (the
/// detail page polls that every 5 seconds) and the apply's own re-check —
/// reachable in the first minute of an engine's life, exactly when an admin
/// adds catalogs (round-2 review finding N1). `deleting` is the one
/// transition that actually means "the cluster this apply is about to touch
/// may already be gone"; every other transition (`provisioning → running`,
/// `running → provisioning` on a later apply, etc.) is a live engine that
/// this apply is still allowed to reach.
pub(crate) async fn apply_built(
    st: &CpState,
    prov: &dyn egret_engine_mgmt_api::EngineProvisioningPort,
    engine: &Engine,
    built: &BuiltReq,
    op: ApplyOp,
) -> Result<(), ApplyBuiltError> {
    match st.engine_mgmt.get_engine(engine.id).await {
        Ok(Some(current)) if current.status != crate::engine_status::DELETING => {}
        _ => {
            // N2 (round-2 review): nothing has been applied at this point —
            // this guard runs BEFORE the provisioner is ever touched — so a
            // pair `build_provision_req_with` minted for this attempt
            // (`built.internal`, present only when it minted a fresh one; a
            // reused recorded pair is `fresh: false` and this is a no-op) is
            // safe to discard HERE and ONLY here: once a request reaches the
            // provisioner, `commit_internal_secrets` takes over and must
            // record it instead, because a failed-but-possibly-delivered
            // apply may have already handed it to the cluster (see that
            // function's doc comment). Discarding on this path prevents an
            // orphaned secret-store row nothing will ever reference — the row
            // this apply was building against is gone or already `deleting`,
            // so no other apply for this engine id can pick it up either.
            if let Some(secrets) = &built.internal {
                discard_internal_secrets(st, secrets).await;
            }
            return Err(ApplyBuiltError::Conflict);
        }
    }
    let res = match op {
        ApplyOp::Provision => prov.provision(&built.req).await,
        ApplyOp::Update => prov.update(&built.req).await,
    };
    // Every attachment this request rendered is now on the engine (Trino renders
    // them all on every apply), so their failed flags — Retry's subject — clear.
    if res.is_ok() && !engine.is_starrocks() {
        if let Some(read_at) = built.catalogs_read_at {
            if let Err(e) = st
                .engine_mgmt
                .registry()
                .mark_engine_applied(engine.id, read_at)
                .await
            {
                tracing::warn!(engine = %engine.name, error = %e,
                    "applied, but the catalog apply status could not be recorded");
            }
        }
    }
    if res.is_ok() {
        retire_ranger_previous_after_apply(st, engine.id, &built.req).await;
    }
    if let Some(secrets) = &built.internal {
        let superseded = if res.is_ok() {
            Supersede::Delete
        } else {
            Supersede::Keep
        };
        if let Err(e) = commit_internal_secrets(st, engine, secrets, superseded).await {
            tracing::error!(engine = %engine.name, error = ?e,
                "minted internal secrets but could not record them; \
                 the monitor fails until the next successful apply");
        }
    }
    res.map_err(ApplyBuiltError::Failed)
}

/// The Ranger binding of an engine bound to `service_name` (`None`: unbound,
/// no binding and no credential), carrying the engine's plugin credential:
/// the committed one ([`crate::ranger_credential::ensure`]), or a new one when
/// `rotate` ([`crate::ranger_credential::rotate`], 409 while a previous one is
/// still pending). Either way the row names the credential BEFORE the apply that
/// renders it.
async fn ranger_binding(
    st: &CpState,
    engine: &Engine,
    service_name: Option<String>,
    rotate: bool,
) -> Result<Option<egret_engine_mgmt_api::RangerBinding>, CpError> {
    let Some(service_name) = service_name else {
        return Ok(None);
    };
    let credential = if rotate {
        crate::ranger_credential::rotate(st, engine).await?
    } else {
        crate::ranger_credential::ensure(st, engine).await?
    };
    // Checked in every build: a credential the renderer would have to escape
    // (only a pre-Task-6 one can be: `mint_password` re-draws a `--`) fails
    // the apply loudly instead of rendering. Rotating mints a renderable one.
    let credential =
        egret_engine_mgmt_api::ranger_config::RenderableCredential::new(
            credential.username,
            credential.password,
        )
        .map_err(|e| {
            tracing::error!(engine = %engine.name, error = %e,
                "the engine's ranger credential cannot be rendered; rotate it");
            CpError::Conflict(format!(
                "engine {}: {e}. Rotate it with POST /api/control-plane/engines/{}/rotate-ranger-credential",
                engine.name, engine.id
            ))
        })?;
    Ok(Some(egret_engine_mgmt_api::RangerBinding {
        service_name,
        egret_url: st.engine_egret_url.clone(),
        poll_interval_ms: egret_engine_mgmt_api::ranger_config::RANGER_POLL_INTERVAL_MS,
        credential: Some(credential),
    }))
}

/// After a SUCCESSFUL apply of `req` — i.e. Kubernetes ACCEPTED it, which can be
/// before the pods have finished rolling — retire the previous credential, by
/// exact id, and only while the credential `req` rendered is still the row's
/// current one
/// ([`crate::ranger_credential::RangerCredentialStore::retire_previous_rendered`]).
/// Never called after a failed apply: the cluster may still hold the previous
/// credential (standing rule).
pub(crate) async fn retire_ranger_previous_after_apply(
    st: &CpState,
    engine_id: Uuid,
    req: &EngineProvisionReq,
) {
    if let Some(credential) = req.ranger.as_ref().and_then(|r| r.credential.as_ref()) {
        crate::ranger_credential::retire_previous_rendered(st, engine_id, credential.password())
            .await;
    }
}

/// `CpError::Conflict` with the wording every apply route uses when
/// [`apply_built`] reports a lost race against a concurrent status change.
pub(crate) fn apply_conflict() -> CpError {
    CpError::Conflict("engine changed while the update was being applied; retry".into())
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
/// `None` and the provisioner applies its default).
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
        // Per-role pod shape (chart `coordinator.*` / `worker.*`).
        init_containers: parse_mini_containers(&v["initContainers"]),
        sidecar_containers: parse_mini_containers(&v["sidecarContainers"]),
        config_mounts: parse_object_mounts(&v["configMounts"]),
        secret_mounts: parse_object_mounts(&v["secretMounts"]),
        share_process_namespace: v["shareProcessNamespace"].as_bool(),
    }
}

/// Back-compat: an engine stored before per-role pod shape carries these fields
/// at the TOP level of `trinoConfig`. Apply a legacy cluster-wide value to a role
/// only when that role has no per-role value of its own.
fn apply_legacy_pod_shape(role: &mut RoleConfigReq, v: &serde_json::Value) {
    if role.init_containers.is_empty() {
        role.init_containers = parse_mini_containers(&v["initContainers"]);
    }
    if role.sidecar_containers.is_empty() {
        role.sidecar_containers = parse_mini_containers(&v["sidecarContainers"]);
    }
    if role.config_mounts.is_empty() {
        role.config_mounts = parse_object_mounts(&v["configMounts"]);
    }
    if role.secret_mounts.is_empty() {
        role.secret_mounts = parse_object_mounts(&v["secretMounts"]);
    }
    if role.share_process_namespace.is_none() {
        role.share_process_namespace = v["shareProcessNamespace"].as_bool();
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
/// wins verbatim downstream in the provisioner; here it just carries through.
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
/// The image a stored engine spec pins, at `path`. An engine with none is an
/// error naming it, never the current default: since migration 0185 every
/// provisioned engine carries its image, so a missing one is an unforeseen row,
/// and defaulting it would upgrade it silently.
fn stored_image(engine: &Engine, path: &[&str]) -> Result<String, CpError> {
    let mut v = Some(&engine.spec);
    for k in path {
        v = v.and_then(|v| v.get(*k));
    }
    v.and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            CpError::BadRequest(format!(
                "engine \"{}\" has no `{}` in its spec; set the image explicitly \
                 (egret will not default it to the current release)",
                engine.name,
                path.join(".")
            ))
        })
}

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
    // Per-role pod shape; legacy top-level values fall back onto both roles.
    let mut coordinator = parse_role(&v["coordinator"], 1);
    apply_legacy_pod_shape(&mut coordinator, v);
    let mut worker_base = parse_role(worker_v, 1);
    apply_legacy_pod_shape(&mut worker_base, v);

    TrinoConfigReq {
        query_max_memory: v["queryMaxMemory"].as_str().map(Into::into),
        log_level: v["logLevel"].as_str().map(Into::into),
        config_properties: str_map(&v["configProperties"]),
        env: str_map(&v["env"]),
        coordinator,
        worker: WorkerRoleConfigReq {
            base: worker_base,
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
        // Trimmed, and blank means "no override" (run `trinodb/trino:{imageVersion}`),
        // exactly as `egret_perm::stored_engine_image` reads it — the image egret
        // reports and judges is the image this renders.
        image: v["image"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(Into::into),
        image_pull_secrets: v["imagePullSecrets"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        image_pull_secret_ref: v["imagePullSecretRef"]
            .as_str()
            .and_then(|s| s.parse().ok()),
        image_pull_secret_json: None,
        image_pull_policy: v["imagePullPolicy"].as_str().map(Into::into),
        service_account: v["serviceAccount"].as_str().map(Into::into),
        pod_security_context: parse_security_context(&v["podSecurityContext"]),
        container_security_context: parse_security_context(&v["containerSecurityContext"]),
        env_from: parse_env_from(&v["envFrom"]),
        deployment_strategy: parse_deployment_strategy(&v["deploymentStrategy"]),
        deployment_annotations: str_map(&v["deploymentAnnotations"]),
        revision_history_limit: v["revisionHistoryLimit"].as_u64().map(|n| n as u32),
        progress_deadline_seconds: v["progressDeadlineSeconds"].as_u64().map(|n| n as u32),
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
/// provisioner never sees plaintext, and an empty `jwksUri` falls back to the
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
            // platform default JWKS above (egret's own machine-IdP, derived from
            // `catalog.public_url`), so this fires only when BOTH the engine block
            // has no `jwksUri` AND the platform has no machine-IdP JWKS configured.
            if jwks_uri.is_empty() {
                return Err(CpError::BadRequest(
                    "Cannot enable engine JWT authentication: the platform has no \
                     machine-IdP JWKS configured. Set `catalog.public_url`, or set a \
                     per-engine `jwksUri`, or disable JWT on this engine."
                        .into(),
                ));
            }
            Some(JwtReq {
                jwks_uri,
                // Unified STS audience (Task 3): egret's machine-IdP mints every
                // engine token with `aud:"engine"`. Default to that so a Trino
                // coordinator validates the SAME audience StarRocks' JWT
                // integration requires unconditionally; an explicit per-engine
                // override is still honored.
                required_audience: Some(
                    j["requiredAudience"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map(String::from)
                        .unwrap_or_else(|| "engine".to_string()),
                ),
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
/// coordinator that has auth enabled.
///
/// The legacy password, derived from the engine id. Valid ONLY for an engine
/// whose row has no `monitor_secret_id`: its pods were rendered before internal
/// secrets were stored and still carry this value in `password.db`. Everything
/// else must go through [`monitor_password_for`].
pub(crate) fn legacy_monitor_password(id: Uuid) -> String {
    format!("egret-monitor-{id}")
}

/// The `egret-monitor` password to present to this engine's coordinator: the
/// stored secret once an apply has recorded one, else the legacy value.
pub(crate) async fn monitor_password_for(st: &CpState, engine: &Engine) -> Result<String, CpError> {
    match engine.monitor_secret_id {
        Some(id) => st
            .secrets
            .resolve_plaintext(None, id)
            .await
            .map_err(|e| CpError::BadRequest(format!("resolve monitor secret: {e}"))),
        None => Ok(legacy_monitor_password(engine.id)),
    }
}

/// Inject the `egret-monitor` password entry into an engine's auth block so the
/// native monitor can authenticate. No-op when `auth` is `None`: this must NOT
/// turn auth on for engines that have none — it only rides along when auth is
/// already enabled. Uses the same `$2b$`→`$2y$` relabel as the human users so
/// Trino's jBCrypt file authenticator accepts the hash.
fn inject_monitor_user(auth: &mut Option<AuthReq>, password: &str) -> Result<(), CpError> {
    let Some(auth) = auth.as_mut() else {
        return Ok(());
    };
    let bcrypt_hash = bcrypt::hash(password, bcrypt::DEFAULT_COST)
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

/// A Trino engine's coordinator↔worker shared secret (`TRINO_INTERNAL_SECRET`)
/// and `egret-monitor` password, as resolved for ONE apply.
struct EngineInternalSecrets {
    internal_id: Uuid,
    internal: String,
    monitor_id: Uuid,
    monitor: String,
    /// Minted for this apply and not yet on the engine row. [`apply_built`]
    /// records it after Kubernetes accepted the spec and deletes it otherwise,
    /// so the row only ever names what the running pods were given.
    fresh: bool,
}

/// Redacts the plaintext fields — this struct carries live credentials and
/// must never show up verbatim in a log line via a stray `{:?}`.
impl std::fmt::Debug for EngineInternalSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineInternalSecrets")
            .field("internal_id", &self.internal_id)
            .field("internal", &"<redacted>")
            .field("monitor_id", &self.monitor_id)
            .field("monitor", &"<redacted>")
            .field("fresh", &self.fresh)
            .finish()
    }
}

/// 32 random bytes, URL-safe base64 without padding — no `:`, so it is also
/// safe as a Basic-auth password.
pub(crate) fn random_internal_secret() -> String {
    use base64::Engine as _;
    use rand::RngCore;
    let mut b = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut b);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

/// Resolve both secrets of an already-recorded pair. `Ok(None)` when either
/// has been deleted out-of-band (e.g. an admin deleting the secret row
/// directly) rather than failing every future apply on this engine forever —
/// the caller mints a fresh pair instead.
async fn resolve_stored_pair(
    st: &CpState,
    internal_id: Uuid,
    monitor_id: Uuid,
) -> Result<Option<EngineInternalSecrets>, CpError> {
    let internal = match st.secrets.resolve_plaintext(None, internal_id).await {
        Ok(v) => v,
        Err(egret_secrets::SecretsError::NotFound) => return Ok(None),
        Err(e) => return Err(CpError::BadRequest(format!("resolve internal secret: {e}"))),
    };
    let monitor = match st.secrets.resolve_plaintext(None, monitor_id).await {
        Ok(v) => v,
        Err(egret_secrets::SecretsError::NotFound) => return Ok(None),
        Err(e) => return Err(CpError::BadRequest(format!("resolve monitor secret: {e}"))),
    };
    Ok(Some(EngineInternalSecrets {
        internal_id,
        internal,
        monitor_id,
        monitor,
        fresh: false,
    }))
}

/// Reuse the engine's recorded internal secrets, or mint a new pending pair when
/// the row has none, `rotate` is set, or a recorded secret has vanished
/// out-of-band. Minting never writes the row.
async fn resolve_internal_secrets(
    st: &CpState,
    engine: &Engine,
    rotate: bool,
) -> Result<EngineInternalSecrets, CpError> {
    if !rotate {
        if let (Some(internal_id), Some(monitor_id)) =
            (engine.internal_secret_id, engine.monitor_secret_id)
        {
            if let Some(existing) = resolve_stored_pair(st, internal_id, monitor_id).await? {
                return Ok(existing);
            }
            // One or both rows are gone (deleted out-of-band). Fall through and
            // mint a fresh pair rather than bricking every future apply.
        }
    }
    // A per-generation suffix keeps a rotation from colliding with the names of
    // the secrets it replaces (those stay until the new apply commits).
    let generation = Uuid::new_v4().simple();
    let internal = random_internal_secret();
    let internal_id = st
        .secrets
        .create(
            None,
            &format!("engine-internal-{}-{generation}", engine.id),
            Some("Trino internal-communication shared secret"),
            &internal,
        )
        .await
        .map_err(|e| CpError::BadRequest(format!("store internal secret: {e}")))?;
    let monitor = random_internal_secret();
    let monitor_id = match st
        .secrets
        .create(
            None,
            &format!("engine-monitor-{}-{generation}", engine.id),
            Some("egret-monitor password for the native Trino monitor"),
            &monitor,
        )
        .await
    {
        Ok(id) => id,
        Err(e) => {
            if let Err(del) = st.secrets.delete(None, internal_id).await {
                tracing::warn!(engine = %engine.name, secret_id = %internal_id, error = %del,
                    "could not delete a half-minted internal secret");
            }
            return Err(CpError::BadRequest(format!("store monitor secret: {e}")));
        }
    };
    Ok(EngineInternalSecrets {
        internal_id,
        internal,
        monitor_id,
        monitor,
        fresh: true,
    })
}

/// Whether [`commit_internal_secrets`], after winning its compare-and-set, may
/// delete the pair it superseded. `Delete` only when the apply that rendered
/// the new pair actually reached the cluster successfully — see
/// [`apply_built`]'s doc comment for why a failed apply passes `Keep` instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Supersede {
    Delete,
    Keep,
}

/// Record a fresh pair on the row — called on BOTH the success and the
/// failure path of an apply that rendered one (see [`apply_built`]) — and,
/// when `superseded` is [`Supersede::Delete`], delete the pair it superseded:
/// `engine.internal_secret_id`/`engine.monitor_secret_id`, i.e. the exact ids
/// the compare-and-set used as `expected`. `engine` is the row as loaded
/// BEFORE the build (its ids are the ones the caller's build read). No-op for
/// a reused pair.
///
/// `Supersede::Keep` (a failed apply) still commits the new pair onto the row
/// — that part is unconditional, see `apply_built` — but deletes nothing: the
/// cluster may be trusting the pair this call is about to supersede, so
/// nothing is destroyed until a LATER successful apply confirms convergence.
///
/// `Supersede::Delete` (a successful apply) deletes ONLY
/// `engine.internal_secret_id`/`engine.monitor_secret_id` — the pair this
/// exact commit displaces — never anything wider. Round 1 of this fix swept
/// EVERY other secret row matching this engine's name prefix on a successful
/// commit, reasoning that a success proves convergence on the new pair; that
/// was wrong; a re-review reproduced the failure: `A` and `B` both build
/// concurrently, `A` commits successfully and swept `B`'s already-minted (but
/// not yet committed) pair — deleting a secret the cluster's next reconcile
/// with `B`'s in-flight request could still be about to receive, in direct
/// violation of the standing "never delete a secret the cluster may still
/// hold" rule (see `apply_built`'s doc comment). Only the CAS's own
/// `expected` pair is provably superseded by THIS commit; any other row
/// (e.g. one a run of `Keep`s left behind, or one a losing concurrent commit
/// left orphaned below) may belong to an apply still in flight and must be
/// left alone here.
///
/// **Consequence — those orphans are a known, intentional gap, not a bug**:
/// a pair left behind by a `Keep` (or by a lost CAS below) is referenced by
/// nothing once a LATER apply supersedes the row again, and nothing in this
/// module ever sweeps it — no concurrent apply on the same engine can be
/// ruled out at any point `commit_internal_secrets` runs, so no prefix-wide
/// delete is ever safe here. The one place that IS safe is engine delete
/// (Task 15): by then the row and every apply path to it are gone, so
/// nothing can still be "in flight" for this engine. Task 15 must (a) delete
/// the row's current `internal_secret_id`/`monitor_secret_id`, and (b)
/// prefix-sweep any remaining `engine-internal-{id}-*` / `engine-monitor-
/// {id}-*` rows this function's `Keep` path left behind.
///
/// When the compare-and-set loses (a concurrent apply on the same engine
/// already committed a different pair since `engine` was read), this writes
/// nothing and does NOT delete `s` either, regardless of `superseded`: `s` may
/// have just been applied to the cluster by THIS call's own
/// `provision`/`update`, so deleting it would risk the same "delete a secret
/// the cluster may hold" failure `apply_built` avoids. It is left as an
/// orphaned secret-store row — see the "Consequence" note above; only Task
/// 15's engine-delete sweep reclaims it.
async fn commit_internal_secrets(
    st: &CpState,
    engine: &Engine,
    s: &EngineInternalSecrets,
    superseded: Supersede,
) -> Result<(), CpError> {
    if !s.fresh {
        return Ok(());
    }
    let committed = st
        .engine_mgmt
        .set_internal_secret_ids(
            engine.id,
            engine.internal_secret_id,
            engine.monitor_secret_id,
            s.internal_id,
            s.monitor_id,
        )
        .await?;
    if !committed {
        tracing::warn!(engine = %engine.name, internal_id = %s.internal_id, monitor_id = %s.monitor_id,
            "lost the race to record internal secrets; a concurrent apply on this engine \
             already committed a different pair — this pair is left unreferenced");
        return Ok(());
    }
    if superseded == Supersede::Keep {
        return Ok(());
    }
    for old in [engine.internal_secret_id, engine.monitor_secret_id]
        .into_iter()
        .flatten()
    {
        if old != s.internal_id && old != s.monitor_id {
            if let Err(e) = st.secrets.delete(None, old).await {
                tracing::warn!(engine = %engine.name, secret_id = %old, error = %e,
                    "could not delete a superseded internal secret");
            }
        }
    }
    Ok(())
}

/// Delete a freshly-minted pair that was never handed to the provisioner at
/// all — the ONLY safe place to discard, because nothing has been applied to
/// the cluster yet. Used when minting succeeds but a later step BEFORE any
/// `provision`/`update` call fails: either a later step in the SAME build
/// (e.g. [`inject_monitor_user`] erroring on a malformed password), or
/// [`apply_built`]'s own pre-provisioner status re-check losing a race (round-2
/// review, N2). Once a request has actually been handed to the provisioner,
/// [`apply_built`]/[`commit_internal_secrets`] takes over and never discards —
/// see their doc comments for why. No-op for a reused pair.
async fn discard_internal_secrets(st: &CpState, s: &EngineInternalSecrets) {
    if !s.fresh {
        return;
    }
    for id in [s.internal_id, s.monitor_id] {
        if let Err(e) = st.secrets.delete(None, id).await {
            tracing::warn!(secret_id = %id, error = %e,
                "could not delete an unapplied internal secret");
        }
    }
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

/// Projection primitive: resolve a `kubernetes.io/dockerconfigjson` named secret
/// to its `.dockerconfigjson` payload. Reused wherever a typed image-pull secret
/// backs a k8s `imagePullSecrets` entry.
async fn resolve_dockerconfigjson_secret(
    st: &CpState,
    secret_ref: Uuid,
) -> Result<String, CpError> {
    let ts = st
        .secrets
        .resolve_typed(None, secret_ref)
        .await
        .map_err(|e| CpError::BadRequest(format!("resolve dockerconfigjson secret: {e}")))?;
    if ts.r#type != egret_secrets::secret_type::DOCKER_CONFIG_JSON {
        return Err(CpError::BadRequest(format!(
            "secret is not a {} secret",
            egret_secrets::secret_type::DOCKER_CONFIG_JSON
        )));
    }
    ts.data
        .get(".dockerconfigjson")
        .filter(|v| !v.is_empty())
        .cloned()
        .ok_or_else(|| {
            CpError::BadRequest("dockerconfigjson secret missing .dockerconfigjson".into())
        })
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

/// Engine usernames of every platform admin (deduped). Unresolvable usernames
/// are skipped; an empty list is valid (nothing to seed).
async fn admin_usernames(st: &CpState) -> Result<Vec<String>, CpError> {
    let ids = st
        .roles
        .platform_admin_user_ids()
        .await
        .map_err(|e| CpError::BadRequest(format!("list platform admins: {e}")))?;
    let mut names = Vec::new();
    for id in ids {
        if let Ok(u) = st.users.ensure_username(id).await {
            if !names.contains(&u) {
                names.push(u);
            }
        }
    }
    Ok(names)
}

/// Seed the delegator's impersonate policy (`egret_delegator` may become any
/// user) onto the engine's bound Trino service. Impersonate engines only; the
/// per-user baseline is [`seed_user_baseline`]'s. No-op when the service does
/// not exist. Upserted, so it is idempotent and lands on a profile the SP4a
/// compiler has claimed too (`create_policy` refuses those, live finding B-F2).
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
    st.authz
        .upsert_infra_policy(
            svc.id,
            dp::IMPERSONATE_POLICY_NAME,
            &dp::delegator_impersonate_policy(service_name),
        )
        .await
        .map_err(|e| CpError::BadRequest(format!("seed policy: {e}")))?;
    recompile_after_infra_seed(st, svc.id).await;
    Ok(())
}

/// Seed the per-user baseline ([`crate::delegator_policy::user_baseline_policies`])
/// onto a delegating Trino engine's bound service: the three per-user checks
/// Trino 483 makes on every session, without which every delegated query fails
/// (DG Task 6b, live result §4.8, finding B-F1). JWT and impersonate engines
/// alike. No-op when the service does not exist.
///
/// Upserted as infrastructure (`managed_by = 'infra'`), like the admin
/// all-access set: idempotent, the compiler's sweep never touches it (it reaps
/// only its own `perm-*` names), and the compiled-profile guard on
/// `create_policy` does not refuse it.
pub(crate) async fn seed_user_baseline(st: &CpState, service_name: &str) -> Result<(), CpError> {
    use crate::delegator_policy as dp;
    let Some(svc) = st
        .authz
        .get_service_opt(service_name)
        .await
        .map_err(|e| CpError::BadRequest(format!("resolve ranger service: {e}")))?
    else {
        return Ok(());
    };
    for (name, body) in dp::user_baseline_policies(service_name) {
        st.authz
            .upsert_infra_policy(svc.id, name, &body)
            .await
            .map_err(|e| CpError::BadRequest(format!("seed policy {name}: {e}")))?;
    }
    recompile_after_infra_seed(st, svc.id).await;
    Ok(())
}

/// Recompile a profile egret has just seeded infra policies on, when its
/// engines carry impersonation grants: a grant's target is re-checked against
/// every policy of the profile (engine grants spec E11, security re-review
/// M-a). The seeded users (platform admins, `egret_delegator`, `{USER}`) are
/// refused as targets already, so this only re-applies a decision the check
/// has made; it runs after the seed (the seed's upserts are not one
/// transaction), fire-and-forget and logged with the repair URL.
async fn recompile_after_infra_seed(st: &CpState, service_id: i64) {
    let Some(compiler) = st.perm_compiler.clone() else {
        return;
    };
    let carries: Result<bool, sqlx::Error> = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM perm.engine_grants g \
           JOIN engine_mgmt.engines e ON e.id = g.engine_id \
          WHERE e.authz_service_id = $1 AND g.target_user_id IS NOT NULL)",
    )
    .bind(service_id)
    .fetch_one(st.engine_mgmt.pool())
    .await;
    match carries {
        Ok(false) => {}
        Ok(true) => {
            if let Err(e) = compiler.recompile_profile(service_id).await {
                tracing::error!(service_id, error = %e,
                    "could not recompile a profile after seeding its infra policies; its \
                     impersonation grants are re-checked at the next recompile, POST \
                     /api/control-plane/perm/compile/{service_id}");
            }
        }
        Err(e) => tracing::error!(service_id, error = %e,
            "could not tell whether a seeded profile carries impersonation grants"),
    }
}

/// Seed the per-user baseline on ONE existing engine, without an apply.
/// Returns whether it seeded: `false` for an external engine, for one that is
/// not a delegating Trino engine, and for one bound to no Ranger profile.
///
/// External engines are refused first and explicitly. egret runs nothing on
/// that cluster, and the apply path never seeds one: every apply handler
/// refuses it, and the build stops at the missing image. But its stored spec
/// is `{}`, which `delegation_mode()` reads as `impersonate`, so
/// `supports_delegation()` alone would let this pass write three allows onto
/// a profile the external cluster's plugin polls.
pub async fn seed_engine_user_baseline(st: &CpState, engine: &Engine) -> Result<bool, CpError> {
    if crate::external::is_external(engine)
        || engine.is_starrocks()
        || !engine.supports_delegation()
    {
        return Ok(false);
    }
    let Some(service_name) = resolve_ranger_service_name(st, engine.authz_service_id).await? else {
        return Ok(false);
    };
    seed_user_baseline(st, &service_name).await?;
    Ok(true)
}

/// The startup pass that brings EVERY existing delegating Trino engine's
/// profile up to the per-user baseline, so engines deployed before the
/// baseline existed work without a manual reapply. Policies are served by
/// egret's own download API, which each plugin polls, so a seeded row reaches
/// the running engine on its next poll; nothing in the cluster is touched.
///
/// Best-effort per engine: a failure is logged with the engine's name and the
/// pass goes on, so one bad profile cannot stop egret from starting. Returns
/// `(seeded, failed)`.
pub async fn seed_user_baselines_for_existing_engines(st: &CpState) -> (usize, usize) {
    let engines = match st.engine_mgmt.list_engines().await {
        Ok(engines) => engines,
        Err(e) => {
            tracing::error!(error = %e, "per-user baseline pass: listing engines failed");
            return (0, 1);
        }
    };
    let (mut seeded, mut failed) = (0, 0);
    for engine in &engines {
        match seed_engine_user_baseline(st, engine).await {
            Ok(true) => seeded += 1,
            Ok(false) => {}
            Err(e) => {
                failed += 1;
                tracing::error!(engine = %engine.name, error = %e,
                    "per-user baseline pass: seeding failed");
            }
        }
    }
    (seeded, failed)
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
    // The stored `status` only advances when `get_engine` observes all pods
    // ready, so recompute the LIVE phase per engine from the provisioner (same
    // source `get_engine` uses), falling back to the stored value if the live
    // read fails or no provisioner is configured.
    let engines = st.engine_mgmt.list_engines().await?;
    let prov = st.engine_provisioner.as_ref();
    let mut out = Vec::with_capacity(engines.len());
    for engine in engines {
        let (kind, ns, name) = (
            engine.r#type.clone(),
            engine.namespace.clone(),
            engine.name.clone(),
        );
        // An external engine has no cluster egret can ask: its row's `running`
        // is a placeholder and `origin` is what a reader shows.
        let external = crate::external::is_external(&engine);
        let mut dto: EngineDto = engine.into();
        if external {
            out.push(dto);
            continue;
        }
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
pub(crate) async fn ensure_engine_role(st: &CpState, warehouse_id: Uuid) -> Result<Uuid, CpError> {
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

/// What an engine create has minted so far; [`compensate_failed_create`]
/// undoes exactly this and nothing else.
#[derive(Debug, Default)]
struct CreateUndo {
    /// The auto-wired Catalog Client.
    client_id: Option<Uuid>,
    /// The named secret holding that client's credential.
    credential_secret_id: Option<Uuid>,
    /// The engine row (its owned secrets are read off it at undo time).
    engine_id: Option<Uuid>,
    /// `(kind, namespace, name)` once a provision has been attempted.
    cluster: Option<(String, String, String)>,
    /// A Ranger profile THIS create made (never a pre-existing one).
    created_profile_id: Option<i64>,
    /// A tag service THIS create made (never a pre-existing one).
    created_tag_service_id: Option<i64>,
}

/// Secret rows egret minted for this engine and owns outright. Reused by both
/// [`compensate_failed_create`] (a create that fails) and, per the standing
/// "never delete a secret the cluster may still hold" rule, by engine delete
/// (Task 15) for the exact same set of ids.
/// `tls_cert_secret_ref` is deliberately absent: it points at an admin-provided
/// `kubernetes.io/tls` secret that other engines may share.
fn engine_owned_secret_ids(engine: &Engine) -> Vec<Uuid> {
    [
        engine.delegator_secret_id,
        engine.tls_secret_id,
        engine.root_secret_id,
        engine.internal_secret_id,
        engine.monitor_secret_id,
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// The engine's Ranger plugin credentials (current and previous, see
/// [`crate::ranger_credential`]). Kept OUT of [`engine_owned_secret_ids`] on
/// purpose: that list doubles as `delete_external_engine`'s "an external row
/// holds no secret" guard, and an external engine may carry this credential
/// (spec 2026-10-20-ranger-surface-auth §4.3). The provisioned delete path and
/// create compensation delete these too.
fn engine_ranger_secret_ids(engine: &Engine) -> Vec<Uuid> {
    [engine.ranger_secret_id, engine.ranger_secret_prev_id]
        .into_iter()
        .flatten()
        .collect()
}

/// Undo a failed engine create. Every step logs its own failure and continues,
/// with ONE exception: if the cluster delete itself fails, this stops right
/// there (mirroring the real `DELETE /engines/{id}` handler, which returns
/// `ClusterDeleteFailed` and leaves the row `deleting` the moment its own
/// cluster delete errors, touching nothing else). Order: cluster, row (then
/// the secrets it referenced — a row that cannot be deleted keeps its secrets
/// and is marked `failed`), profiles this create made (their policies
/// cascade), catalog client. Policies seeded into a PRE-EXISTING profile are
/// left alone: other engines may be bound to it.
///
/// Deleting `engine_owned_secret_ids(&engine)` here is safe under the standing
/// "never delete a secret the cluster may still hold" rule because (a) these
/// are secrets THIS create minted (a brand-new engine id no other in-flight
/// create or apply can be racing), and (b) reaching the row/secret deletes
/// below REQUIRES either that the cluster was never touched (the failure hit
/// before `apply_built`/`prov.provision`, so `undo.cluster` is `None`) or that
/// its delete just succeeded — see the early return below for the third case.
/// So whatever the apply rendered from these secrets into the cluster is
/// either confirmed gone or was never written, before this ever removes the
/// secret-store rows backing it; nothing here can outlive its cluster copy.
/// This includes `internal_secret_id`/`monitor_secret_id`: `commit_internal_secrets`
/// commits a freshly-minted pair onto the row on BOTH the success and the
/// failure path of an apply that rendered one, so by the time compensation
/// reads the row those ids are exactly the pair (if any) this create's own,
/// and only this create's own, apply attempted — never one a concurrent apply
/// might still be relying on.
///
/// When the cluster delete FAILS, the opposite must hold: the apply may have
/// already written live cluster objects from these very secrets (e.g. the
/// Trino env Secret rendering `TRINO_INTERNAL_SECRET`, or the password.db
/// `ConfigMap`) that this call could not remove. Deleting the row or its
/// secrets at that point would orphan those objects with nothing left to name
/// them and no way back in through the API — kubectl would be the only route
/// to clean them up. So on that path this marks the row `failed` (§4.7) and
/// returns immediately, deleting nothing: the row stays visible and
/// deletable, and a later `DELETE /engines/{id}` retries the SAME cluster
/// delete this call just attempted, then does its own row/secret/client
/// cleanup once that retry succeeds.
async fn compensate_failed_create(st: &CpState, undo: CreateUndo) {
    if let (Some((kind, namespace, name)), Some(prov)) =
        (&undo.cluster, st.engine_provisioner.as_ref())
    {
        if let Err(e) = prov.delete(kind, namespace, name).await {
            tracing::error!(engine = %name, error = %e,
                "create compensation: cluster delete failed; row kept (marked failed) \
                 so DELETE /engines/{{id}} can retry it — no row or secret removed");
            if let Some(engine_id) = undo.engine_id {
                if let Err(e) = st
                    .engine_mgmt
                    .set_engine_status(engine_id, crate::engine_status::FAILED)
                    .await
                {
                    tracing::error!(%engine_id, error = %e,
                        "create compensation: could not mark the engine failed");
                }
            }
            return;
        }
    }
    let mut row_kept = false;
    if let Some(engine_id) = undo.engine_id {
        match st.engine_mgmt.get_engine(engine_id).await {
            Ok(Some(engine)) => {
                let mut owned = engine_owned_secret_ids(&engine);
                owned.extend(engine_ranger_secret_ids(&engine));
                match st.engine_mgmt.delete_engine(engine_id).await {
                    Ok(()) => {
                        for secret_id in owned {
                            if let Err(e) = st.secrets.delete(None, secret_id).await {
                                tracing::error!(engine = %engine.name, %secret_id, error = %e,
                                    "create compensation: secret delete failed");
                            }
                        }
                    }
                    Err(e) => {
                        row_kept = true;
                        tracing::error!(engine = %engine.name, error = %e,
                            "create compensation: engine row delete failed; marking it failed");
                        if let Err(e) = st
                            .engine_mgmt
                            .set_engine_status(engine_id, crate::engine_status::FAILED)
                            .await
                        {
                            tracing::error!(engine = %engine.name, error = %e,
                                "create compensation: could not mark the engine failed");
                        }
                    }
                }
            }
            Ok(None) => {}
            Err(e) => tracing::error!(%engine_id, error = %e,
                "create compensation: could not load the engine row"),
        }
    }
    // The credential of the auto-wired client. Kept only while the row is kept
    // (its attachment still names it); otherwise the cluster is confirmed gone or
    // was never touched, exactly like the owned secrets above.
    if !row_kept {
        if let Some(secret_id) = undo.credential_secret_id {
            if let Err(e) = st.secrets.delete(None, secret_id).await {
                tracing::error!(%secret_id, error = %e, "create compensation: credential secret delete failed");
            }
        }
    }
    for (what, id) in [
        ("policy profile", undo.created_profile_id),
        ("tag service", undo.created_tag_service_id),
    ] {
        if let Some(id) = id {
            if let Err(e) = st.authz.delete_service(id).await {
                tracing::error!(service_id = id, error = %e,
                    "create compensation: {what} delete failed");
            }
        }
    }
    if let Some(client_id) = undo.client_id {
        if let Err(e) = st.catalog.revoke_client(client_id).await {
            tracing::error!(%client_id, error = ?e,
                "create compensation: catalog client revoke failed");
        }
    }
}

/// Resolve the Ranger service NAME an engine's `authz_service_id` actually
/// names, by reading the bound profile row (`policy.authz_services`) — never
/// by assuming it is spelled like the engine (SP4a Task 3b).
///
/// **Why this exists.** `PUT /engines/{id}` (and create's `authz_service_id`)
/// let an operator bind an engine to a profile whose name differs from the
/// engine's. Deriving the Ranger service name from `engine.name` instead of
/// the bound row silently enforces whatever OTHER profile happens to be named
/// after the engine (or 404s forever if none is), never the one the operator
/// actually bound. That is the defect this closes; every call site that used
/// to write `authz_service_id.map(|_| engine.name.clone())` calls this
/// instead.
///
/// **A bound id with no matching row is a loud failure**, never a silent
/// fall-back to the engine's name — that fallback IS the defect. Ranger
/// profile rows are never deleted by bind/unbind (standing constraint), so
/// this should only ever fire on genuine data corruption; when it does, the
/// caller must see it rather than quietly enforce the wrong (or no) policy.
pub(crate) async fn resolve_ranger_service_name(
    st: &CpState,
    authz_service_id: Option<i64>,
) -> Result<Option<String>, CpError> {
    let Some(sid) = authz_service_id else {
        return Ok(None);
    };
    let services = st
        .authz
        .list_services()
        .await
        .map_err(|e| CpError::BadRequest(format!("list policy profiles: {e}")))?;
    match services.into_iter().find(|s| s.id == sid) {
        Some(svc) => Ok(Some(svc.name)),
        None => {
            tracing::error!(
                service_id = sid,
                "engine is bound to authz_service_id {sid} but no such policy profile row \
                 exists; refusing to fall back to the engine's name"
            );
            Err(CpError::Internal)
        }
    }
}

/// How a new Trino engine gets its Ranger profile — decided before anything is
/// minted, so a conflict is a clean 409.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ProfilePlan {
    /// No binding: Ranger is disabled and no valid profile was chosen. Trino
    /// defaults to allow-all, which is what delegated impersonation needs there.
    Unbound,
    /// Bind to a profile that already exists (chosen, or an orphan left by a
    /// deleted same-name engine). Compensation never deletes it.
    Existing(i64),
    /// Create `<name>` (trino) and `<name>_tag` (tag) during the create.
    Create,
}

/// The 409 for a same-name profile of the wrong type. It says what is wrong and
/// the one remedy that exists as a route: delete that profile.
fn profile_type_conflict(name: &str, found_type: &str) -> String {
    format!(
        "engine {name} cannot take its own policy profile: a profile named {name} already exists \
         with type {found_type}, not trino. If no engine uses it, delete it \
         (DELETE /api/control-plane/policies/services/{name}) and retry"
    )
}

/// The 409 for a same-name profile another engine already binds.
fn profile_bound_conflict(name: &str, engines: &[String]) -> String {
    format!(
        "engine {name} cannot take its own policy profile: profile {name} is already bound to \
         engine(s) {}, and one profile serves one engine. Re-point those engines at another \
         profile (PUT /api/control-plane/engines/{{id}} with authzServiceId) or delete them, \
         then retry",
        engines.join(", ")
    )
}

/// Decide the profile for a Trino create named `name`.
async fn plan_trino_profile(
    st: &CpState,
    name: &str,
    requested: Option<i64>,
) -> Result<ProfilePlan, CpError> {
    if let Some(sid) = requested {
        let services = st.authz.list_services().await.map_err(|e| {
            tracing::error!(error = %e, "listing policy profiles failed");
            CpError::Internal
        })?;
        if services.iter().any(|s| s.id == sid) {
            return Ok(ProfilePlan::Existing(sid));
        }
        tracing::warn!(sid, "authz_service_id not found");
    }
    if !st.ranger_enabled {
        tracing::info!(
            engine = %name,
            "ranger disabled; Trino engine provisioned without Ranger access-control (allow-all)"
        );
        return Ok(ProfilePlan::Unbound);
    }
    match st.authz.get_service_opt(name).await.map_err(|e| {
        tracing::error!(error = %e, "resolving the policy profile failed");
        CpError::Internal
    })? {
        None => Ok(ProfilePlan::Create),
        Some(svc) if svc.r#type != "trino" => {
            Err(CpError::Conflict(profile_type_conflict(name, &svc.r#type)))
        }
        Some(svc) => {
            let bound: Vec<String> = st
                .engine_mgmt
                .list_engines()
                .await?
                .into_iter()
                .filter(|e| e.authz_service_id == Some(svc.id))
                .map(|e| e.name)
                .collect();
            if bound.is_empty() {
                Ok(ProfilePlan::Existing(svc.id))
            } else {
                Err(CpError::Conflict(profile_bound_conflict(name, &bound)))
            }
        }
    }
}

/// Carry out a [`ProfilePlan`] for the new engine row, recording anything it
/// creates in `undo`.
async fn bind_trino_profile(
    st: &CpState,
    engine_id: Uuid,
    name: &str,
    plan: &ProfilePlan,
    undo: &mut CreateUndo,
) -> Result<(), CpError> {
    let profile_id = match plan {
        ProfilePlan::Unbound => return Ok(()),
        ProfilePlan::Existing(id) => *id,
        ProfilePlan::Create => {
            st.authz
                .ensure_trino_def()
                .await
                .map_err(|e| CpError::BadRequest(format!("seed trino service-def: {e}")))?;
            // A TAG service alongside the trino profile (Access P3.2), linked by
            // the profile's `tagService` config, so tag-based masking/row-filter
            // policies have a service. Best-effort: the profile works untagged.
            let tag_service_name = format!("{name}_tag");
            if let Err(e) = st.authz.ensure_tag_def().await {
                tracing::warn!(error = %e, "ensure_tag_def failed during auto-bind");
            }
            let tag_wired = match st
                .authz
                .create_service(&tag_service_name, "tag", &serde_json::json!({}))
                .await
            {
                Ok(svc) => {
                    undo.created_tag_service_id = Some(svc.id);
                    true
                }
                // Left by a deleted same-name engine: reuse, never delete.
                Err(egret_policy_api::PolicyError::AlreadyExists(_)) => true,
                Err(e) => {
                    tracing::warn!(error = %e, "create tag service failed during auto-bind");
                    false
                }
            };
            let config = if tag_wired {
                serde_json::json!({ egret_policy_api::TAG_SERVICE_CONFIG_KEY: tag_service_name })
            } else {
                serde_json::json!({})
            };
            let svc = match st.authz.create_service(name, "trino", &config).await {
                Ok(svc) => svc,
                Err(egret_policy_api::PolicyError::AlreadyExists(_)) => {
                    return Err(CpError::Conflict(format!(
                        "policy profile {name} was created concurrently; retry the create"
                    )))
                }
                Err(e) => return Err(CpError::BadRequest(format!("create policy profile: {e}"))),
            };
            undo.created_profile_id = Some(svc.id);
            svc.id
        }
    };
    // A pair this bind creates is refused (SP4d-4 Task 7 fix round 1, C1): the
    // caller undoes the create, or fails the late-bind apply.
    crate::profile_compiler::bind_profile_guarded(st, engine_id, name, profile_id).await
}

/// Resolve the warehouse the engine leads with, from whichever shape the
/// caller sent, and load the OTHER catalogs to attach afterwards. Every refusal
/// lives here so the Trino and StarRocks create paths cannot disagree about what
/// a valid body is.
///
/// WHY A WAREHOUSE AT ALL. Two pieces of CLUSTER state are derived from the
/// primary attachment's warehouse: `ensure_engine_role` seeds the engine's
/// Ranger role from it, and a StarRocks engine's builtin storage volume gets
/// its S3 coordinates from it ([`primary_warehouse`], which reads the
/// attachment; there is no `engines.warehouse_id` column any more).
/// So a catalog with no warehouse cannot lead, the refusal below NAMES that
/// reason so the wizard can repeat it rather than just greying a radio button,
/// and it is decided HERE — before any registry row is written, so a refused
/// create has nothing to roll back (SP3a F-2).
///
/// Letting a non-warehouse catalog lead is a FEATURE, deferred past SP4 (user
/// decision 2026-09-23): it needs somewhere else for those two facts to come
/// from, not merely a relaxed check here.
///
/// EVERY id IS RESOLVED HERE, BEFORE ANYTHING IS PROVISIONED OR MINTED (review
/// F1). This runs at the top of `create_engine`, ahead of the warehouse lookup,
/// the catalog client, the engine row and the cluster. A typo in the fourth of
/// four catalog names must not leave the caller with a running cluster and a
/// message in an optional field they may never read — it is a refusal, and the
/// same refusal whichever entry carries it. The loaded rows are RETURNED so the
/// attach step afterwards cannot re-resolve them and reach a different answer.
async fn resolve_primary_catalog(
    st: &CpState,
    b: &CreateEngineBody,
) -> Result<(Uuid, Vec<egret_catalog_registry::CatalogRow>), CpError> {
    if b.catalogs.is_empty() {
        return match b.warehouse_id {
            Some(id) => Ok((id, Vec::new())),
            None => Err(CpError::BadRequest(
                "an engine needs a primary catalog: send `catalogs` with one marked primary".into(),
            )),
        };
    }
    // Both shapes at once (review F7). `catalogs` supersedes `warehouseId`, so
    // honouring it and dropping the other silently is how an integrator spends
    // an afternoon wondering which warehouse the engine actually got.
    if b.warehouse_id.is_some() {
        return Err(CpError::BadRequest(
            "send either `warehouseId` or `catalogs`, not both: `catalogs` supersedes \
             `warehouseId` and names the primary itself"
                .into(),
        ));
    }
    let primaries: Vec<&NewEngineCatalog> = b.catalogs.iter().filter(|c| c.primary).collect();
    let primary_id = match primaries.as_slice() {
        [one] => one.catalog_id,
        [] => {
            return Err(CpError::BadRequest(
                "one attached catalog must be primary".into(),
            ))
        }
        _ => {
            return Err(CpError::BadRequest(
                "only one attached catalog may be primary".into(),
            ))
        }
    };
    let registry = st.engine_mgmt.registry();
    let mut seen = std::collections::HashSet::new();
    let mut primary_row = None;
    let mut others = Vec::with_capacity(b.catalogs.len());
    for c in &b.catalogs {
        // A repeated id would attach once and 409 the second time — the same
        // "cluster is up, here is a message" outcome F1 is about.
        if !seen.insert(c.catalog_id) {
            return Err(CpError::BadRequest(format!(
                "catalog {} is listed twice",
                c.catalog_id
            )));
        }
        let row = registry
            .get_catalog(c.catalog_id)
            .await?
            // Named, not a bare 404 (review F9): with four ids in the body, the
            // caller has to be told WHICH one does not exist. Same 400 an
            // unknown `warehouseId` gets, for the same reason — it is the
            // body that is wrong, not the route.
            .ok_or_else(|| CpError::BadRequest(format!("no such catalog {}", c.catalog_id)))?;
        if c.catalog_id == primary_id {
            primary_row = Some(row);
        } else {
            others.push(row);
        }
    }
    // Unreachable: exactly one entry carries `primary`, and every entry was
    // loaded above.
    let primary = primary_row.ok_or(CpError::Internal)?;
    let wh = primary.warehouse_id.ok_or_else(|| {
        CpError::BadRequest(
            "the primary catalog must be an egret warehouse catalog: the engine's Ranger \
             role and storage are derived from its warehouse"
                .into(),
        )
    })?;
    Ok((wh, others))
}

/// Attach the body's non-primary `catalogs` to an engine that now exists,
/// through the SAME [`crate::catalog_apply::attach_to_engine`] the attach route
/// uses — one at a time, exactly as the wizard's own loop would, and never a
/// second attach implementation. Returns what did not attach; the engine is NOT
/// rolled back for it (its cluster is already up).
///
/// The rows come from [`resolve_primary_catalog`], which already refused every
/// id that does not exist, so nothing reachable here is a typo: what remains is
/// an engine that cannot read the catalog, a name StarRocks reserves, or a
/// cluster that refused the apply.
async fn attach_extra_catalogs(
    st: &CpState,
    engine: &Engine,
    catalogs: &[egret_catalog_registry::CatalogRow],
) -> Vec<CatalogAttachErrorDto> {
    let mut failures = Vec::new();
    for catalog in catalogs {
        if let Err(e) = crate::catalog_apply::attach_to_engine(st, engine, catalog, None).await {
            let message = e.into_registry().to_string();
            tracing::warn!(engine = %engine.name, catalog = %catalog.name, %message,
                "engine created, but a catalog from the create body did not attach");
            failures.push(CatalogAttachErrorDto {
                catalog_id: catalog.id,
                message,
            });
        }
    }
    failures
}

/// 400 when a Trino create/update body names an image egret no longer supports
/// (`trinodb/trino:470`, Ranger plugin 2.5.0): such an engine can never hold the
/// credential the Ranger surface requires. Checks the image the body asks for,
/// both `imageVersion` and a `trinoConfig.image` override.
fn refuse_dropped_trino_image(b: &CreateEngineBody) -> Result<(), CpError> {
    let from_version = b
        .image_version
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(egret_perm::trino_image_ref);
    let from_override = b
        .trino_config
        .as_ref()
        .and_then(|tc| tc.get("image"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    for image in from_version.into_iter().chain(from_override) {
        if let Some(why) = egret_perm::unsupported_engine_image("trino", &image) {
            return Err(CpError::BadRequest(why));
        }
    }
    Ok(())
}

#[utoipa::path(
    post,
    path = "/api/control-plane/engines",
    tag = "control-plane-engines",
    request_body = CreateEngineBody,
    responses(
        (status = 201, description = "Engine created and provisioned; the primary catalog is attached, and `catalogAttachErrors` lists any of the request's other `catalogs` the engine came up without", body = CreateEngineResponse),
        (status = 400, description = "Unknown/unprovisioned warehouse, invalid input, or provisioning failed", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:create permission", body = ErrorBody),
        (status = 409, description = "An engine with this name already exists, or its same-name policy profile is bound to another engine", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn create_engine(
    State(st): State<CpState>,
    Auth(p): Auth,
    Json(b): Json<CreateEngineBody>,
) -> Result<(StatusCode, Json<CreateEngineResponse>), CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:create").await?;
    if egret_engine_mgmt_api::EngineKind::parse(&b.kind)
        == Some(egret_engine_mgmt_api::EngineKind::Trino)
    {
        refuse_dropped_trino_image(&b)?;
    }

    // 1. Resolve the warehouse the engine row still needs — from `catalogs`'
    //    primary, or from a legacy bare `warehouseId` — and with it the other
    //    catalogs to attach once the engine is up. ONE refusal point for both
    //    create paths (see `resolve_primary_catalog`). It must exist and be
    //    provisioned (has a lakekeeper counterpart) — engines connect by
    //    warehouse NAME.
    let (primary_warehouse_id, extra_catalogs) = resolve_primary_catalog(&st, &b).await?;
    let wh = st
        .catalog
        .get_warehouse(primary_warehouse_id)
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

    // Validate an optional ingress-TLS secret reference the same way (400 on a
    // missing / non-TLS / incomplete secret) before any resources are minted.
    if let Some(ing) = b
        .trino_config
        .as_ref()
        .and_then(|tc| parse_ingress(&tc["ingress"]))
    {
        if let Some(ref_id) = ing.tls_cert_secret_ref {
            resolve_tls_cert_secret(&st, ref_id).await?;
        }
    }

    // Validate an optional image-pull (dockerconfigjson) secret reference the
    // same way (400 on a missing / non-dockerconfigjson / incomplete secret)
    // before any resources are minted.
    if let Some(ref_id) = b
        .trino_config
        .as_ref()
        .and_then(|tc| tc["imagePullSecretRef"].as_str())
        .and_then(|s| s.parse::<Uuid>().ok())
    {
        resolve_dockerconfigjson_secret(&st, ref_id).await?;
    }

    match egret_engine_mgmt_api::EngineKind::parse(&b.kind) {
        Some(egret_engine_mgmt_api::EngineKind::StarRocks) => {
            return create_starrocks_engine(st, b, wh, extra_catalogs).await;
        }
        Some(egret_engine_mgmt_api::EngineKind::Trino) => {}
        None => {
            return Err(CpError::BadRequest(format!(
                "unknown engine kind: {}",
                b.kind
            )));
        }
    }

    // 2. The warehouse's storage connection must exist (the renderer resolves
    //    its endpoint on every apply).
    st.storage_registry
        .get_storage_conn(wh.storage_conn_id)
        .await?
        .ok_or_else(|| CpError::BadRequest("unknown storage connection".into()))?;

    // 3. Build + resolve the per-role trinoConfig — including default-on
    //    delegation — BEFORE minting the catalog client. Ordering matters: the
    //    delegation resolver 400s on an explicit `jwt` a no-JWKS platform can't
    //    satisfy (or an unknown mode), and if the catalog client were already
    //    minted that early-return would orphan a client row + credentials. Mirror
    //    the StarRocks path (`create_starrocks_engine`), which resolves delegation
    //    before `catalog.create_client`, so a 400 leaves nothing behind.
    //
    //    Seed the per-role trinoConfig from the wizard inputs (both roles share the
    //    cpuMin/cpuMax/memory resources; replicas come from the body). Any
    //    per-role block supplied in the body overlays these seeded defaults.
    let mut trino_config = seed_trino_config(&b);
    if let (Some(seed), Some(supplied)) = (
        trino_config.as_object_mut(),
        b.trino_config.as_ref().and_then(|v| v.as_object()),
    ) {
        for (k, v) in supplied {
            seed.insert(k.clone(), v.clone());
        }
    }
    // Default-on delegation (spec 2026-07-21) — mirror the StarRocks path (Task 2).
    // Resolve from trinoConfig.delegationMode; persist the resolved concrete mode so
    // the stored spec is explicit (build_provision_req/resolve_auth read it back).
    // Create-only; update preserves the stored mode. `trino_config` is always a JSON
    // object (seed_trino_config), so a bare create (no `trinoConfig` in the body)
    // still gets the default injected here. This also VALIDATES the mode (an unknown
    // value / an explicit `jwt` with no platform JWKS 400s here, before any catalog
    // client is minted — see the ordering note above).
    // NOTE: unlike StarRocks (whose absent delegation reads back as "none"), Trino's
    // `store::Engine::delegation_mode()` defaults an ABSENT `delegationMode` to
    // "impersonate" — so the "none" opt-out must be persisted EXPLICITLY as "none",
    // NOT by removing the key (removing it would read back as impersonate).
    {
        let mode = trino_config.get("delegationMode").and_then(|v| v.as_str());
        let resolved = crate::delegation_default::resolve_delegation(
            mode,
            crate::delegation_default::has_platform_jwks(
                &st.engine_jwks_uri,
                &st.engine_oidc_issuer,
            ),
            None, // Trino has no engine-version floor for JWT
        )
        .map_err(CpError::BadRequest)?;
        let obj = trino_config
            .as_object_mut()
            .expect("trinoConfig is a JSON object");
        match resolved {
            crate::delegation_default::ResolvedDelegation::Jwt => {
                obj.insert("delegationMode".into(), serde_json::json!("jwt"));
                // Ensure an auth.jwt object exists so resolve_auth synthesizes the
                // platform JWKS + aud "engine". Don't clobber an admin-provided block.
                let auth = obj.entry("auth").or_insert_with(|| serde_json::json!({}));
                if let Some(a) = auth.as_object_mut() {
                    a.entry("jwt").or_insert_with(|| serde_json::json!({}));
                }
            }
            crate::delegation_default::ResolvedDelegation::Impersonate => {
                obj.insert("delegationMode".into(), serde_json::json!("impersonate"));
            }
            crate::delegation_default::ResolvedDelegation::Open => {
                // Persist "none" EXPLICITLY (absent ⇒ impersonate for Trino).
                obj.insert("delegationMode".into(), serde_json::json!("none"));
            }
        }
    }

    // 4. Assemble and validate the stored spec BEFORE minting anything, so an
    //    invalid resource-group config 400s with nothing to roll back.
    let image_version = b
        .image_version
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_ENGINE_IMAGES.trino_image_version.into());
    let namespace = DEFAULT_NAMESPACE.to_string();
    let mut spec = serde_json::json!({
        "coordinatorReplicas": b.coordinator_replicas,
        "workerReplicas": b.worker_replicas,
        "cpuMin": b.cpu_min,
        "cpuMax": b.cpu_max,
        "memory": b.memory,
        "imageVersion": image_version,
    });
    spec["trinoConfig"] = trino_config;
    validate_spec_resource_groups(&spec)?;
    // Kubernetes must be configured before anything is minted or rendered.
    let Some(prov) = st.engine_provisioner.clone() else {
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };

    // Decide the Ranger profile BEFORE minting anything: a same-name profile
    // bound to another engine is a 409 with nothing to roll back.
    let profile_plan = plan_trino_profile(&st, &b.name, b.authz_service_id).await?;

    // 5. Auto-wire: the warehouse's registry catalog (created with the warehouse;
    //    ensured here for one that predates the registry) and a fresh Catalog
    //    Client for this engine, whose credential lives in a named secret. Minted
    //    only AFTER delegation resolves (step 3) and the spec validates (step 4);
    //    from here on every failure runs `compensate_failed_create`.
    let (wh_catalog, _) = st
        .engine_mgmt
        .registry()
        .ensure_warehouse_catalog_with_compiler(
            &WarehouseIdentity {
                id: wh.id,
                name: &wh.name,
                workspace_id: wh.workspace_id,
            },
            None,
            st.catalog_assignment_compiler.as_deref(),
        )
        .await?;
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
    let mut undo = CreateUndo {
        client_id: Some(client.id),
        ..CreateUndo::default()
    };
    match crate::catalog_apply::store_client_credential(&st, &client).await {
        Ok(id) => undo.credential_secret_id = Some(id),
        Err(e) => {
            compensate_failed_create(&st, undo).await;
            return Err(e);
        }
    }

    // 6. Record the intent row (status `provisioning`).
    let engine = match st
        .engine_mgmt
        .create_engine(&b.name, "trino", &namespace, &spec, b.tls_cert_secret_ref)
        .await
    {
        Ok(e) => e,
        Err(e) => {
            compensate_failed_create(&st, undo).await;
            return Err(e.into());
        }
    };
    undo.engine_id = Some(engine.id);

    // 7. Attach the warehouse's catalog: the engine's PRIMARY catalog. Marked
    //    applied by the provision below.
    if let Err(e) = st
        .engine_mgmt
        .registry()
        .insert_attachment(&egret_catalog_registry::NewAttachment {
            engine_id: engine.id,
            catalog_id: wh_catalog.id,
            catalog_client_id: Some(client.id),
            credential_secret_id: undo.credential_secret_id,
            minted_client: true,
            applied_now: false,
            // The engine's PRIMARY catalog (SP3c D2). `resolve_primary_catalog`
            // has already established which catalog that is, and the partial
            // unique index from migration 0150 refuses a second.
            is_primary: true,
            created_at: None,
            apply_status: egret_catalog_registry::store::apply_status::APPLIED,
        })
        .await
    {
        compensate_failed_create(&st, undo).await;
        return Err(e.into());
    }

    // 8. Bind the Ranger profile BEFORE the first provision, so a governed
    //    engine never runs unbound (allow-all) — not even between two applies.
    if let Err(e) = bind_trino_profile(&st, engine.id, &b.name, &profile_plan, &mut undo).await {
        compensate_failed_create(&st, undo).await;
        return Err(e);
    }
    let engine = match st.engine_mgmt.get_engine(engine.id).await {
        Ok(Some(e)) => e,
        Ok(None) => {
            compensate_failed_create(&st, undo).await;
            return Err(CpError::NotFound);
        }
        Err(e) => {
            compensate_failed_create(&st, undo).await;
            return Err(e.into());
        }
    };

    // 9. Build from the stored (bound) engine + catalogs and provision once.
    let built = match build_provision_req(&st, &engine).await {
        Ok(built) => built,
        Err(e) => {
            compensate_failed_create(&st, undo).await;
            return Err(e);
        }
    };
    undo.cluster = Some((
        engine.r#type.clone(),
        engine.namespace.clone(),
        engine.name.clone(),
    ));
    if let Err(e) = apply_built(&st, prov.as_ref(), &engine, &built, ApplyOp::Provision).await {
        compensate_failed_create(&st, undo).await;
        return Err(match e {
            ApplyBuiltError::Conflict => apply_conflict(),
            ApplyBuiltError::Failed(msg) => {
                CpError::BadRequest(format!("engine provisioning failed: {msg}"))
            }
        });
    }

    // 10. The body's non-primary catalogs, now that the engine exists and its
    //     cluster is up. Past the rollback: a failure here is that catalog's,
    //     not the engine's.
    let catalog_attach_errors = attach_extra_catalogs(&st, &engine, &extra_catalogs).await;

    // 11. Bring the bound profile's compiled `perm` policies up to date
    //     (SP4a Task 9 review). Here, at the END, and not next to
    //     `bind_trino_profile` in step 8: the compiler's desired set is derived
    //     from the catalogs the profile's engines REACH, and the non-primary
    //     ones are not attached until step 10. A recompile at bind time would
    //     see the primary catalog (step 7, before the bind) but none of the
    //     others, and would compile an INCOMPLETE set — a subtler form of the
    //     staleness it exists to remove. Never fails the create; see
    //     `profile_compiler::recompile_bound_profile`.
    crate::profile_compiler::recompile_bound_profile(&st, engine.id, &engine.name).await;

    Ok((
        StatusCode::CREATED,
        Json(CreateEngineResponse {
            engine: engine.into(),
            catalog_attach_errors,
        }),
    ))
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
/// body): `feImage`/`cnImage` (defaulting to `DEFAULT_ENGINE_IMAGES`'s
/// StarRocks images when absent or blank), `feReplicas`/`cnReplicas`
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
    build_starrocks_provision_req_with(st, engine, wh, false).await
}

/// [`build_starrocks_provision_req`], rotating the Ranger plugin credential
/// when `rotate_ranger_credential` (see [`BuildOpts::rotate_ranger_credential`]).
async fn build_starrocks_provision_req_with(
    st: &CpState,
    engine: &Engine,
    wh: &egret_catalog_api::Warehouse,
    rotate_ranger_credential: bool,
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
    let fe_image = stored_image(engine, &["starrocksConfig", "feImage"])?;
    let cn_image = stored_image(engine, &["starrocksConfig", "cnImage"])?;
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
    // FE metadata PVC knobs. Absent/null ⇒ `None` ⇒ default StorageClass +
    // `DEFAULT_FE_META_STORAGE_SIZE` in the provisioner. They only have an
    // effect at all when `feMetaPersistent` below is true: an engine created
    // before the PVC shipped renders NO meta volume whatever these say, and
    // keeps the operator's `emptyDir`.
    let fe_meta_storage_class = cfg
        .and_then(|c| c.get("feMetaStorageClass"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty());
    let fe_meta_storage_size = cfg
        .and_then(|c| c.get("feMetaStorageSize"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty());
    // The creation-time marker. Absent / null / `false` ⇒ `false`, so every
    // engine that predates the marker renders the pre-PVC shape and its
    // StatefulSet keeps reconciling. Only `build_starrocks_spec` on the CREATE
    // path writes `true`; see `StarRocksConfig::fe_meta_persistent` for why
    // this can never be flipped on for a live engine.
    let fe_meta_persistent = cfg
        .and_then(|c| c.get("feMetaPersistent"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Delegation opt-in read back from the stored `{mode, tlsEnabled}` block.
    // `tls_enabled` mirrors create: on only when delegation is configured,
    // defaulting to true (TLS-on) unless the stored block opted out.
    let delegation_cfg = cfg.and_then(|c| c.get("delegation"));
    let delegation_mode = delegation_cfg
        .and_then(|d| d.get("mode"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    let tls_enabled = sr_tls_enabled(&engine.spec);

    // Ranger resolution — the SAME helper `build_provision_req` uses for Trino:
    // resolve the ACTUAL bound profile's name from `policy.authz_services`,
    // never assume it matches the engine's; unbound → None.
    // The binding itself (and its credential) is settled last, just before the
    // request is assembled.
    let ranger_service_name = resolve_ranger_service_name(st, engine.authz_service_id).await?;

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
        // Unified STS audience (Task 3): egret's machine-IdP mints every engine
        // token with `aud:"engine"`, so the JWT integration must require that,
        // not a per-engine-kind literal.
        audience: "engine".into(),
        tls_cert_pem: delegation_cert_pem.clone().unwrap_or_default(),
        tls_keystore_pem: delegation_keystore_pem.clone().unwrap_or_default(),
        tls_enabled,
    });
    let delegation_bootstrap = delegation_mode.map(|_| StarRocksDelegationBootstrap {
        jwks_url: st.engine_jwks_uri.clone(),
        issuer: st.engine_oidc_issuer.clone(),
        audience: "engine".into(),
        // No `tls_cert_pem`: `ensure_tls_cert` above has already persisted the
        // cert on the engine row, and `starrocks_ddl::fe_target` reads it back
        // for EVERY statement the bootstrap sends (D6).
        service_user: crate::delegator_policy::DELEGATOR_USERNAME.into(),
        service_password: delegation_service_password.clone().unwrap_or_default(),
    });

    // Ranger auto-admin (sub-project A): grant platform admins all-access on the
    // bound Ranger service so a governed StarRocks engine is immediately queryable
    // with no manual policy seeding. Idempotent (upsert). Mirrors the Trino path.
    if let Some(service_name) = ranger_service_name.as_deref() {
        if engine.supports_delegation() {
            if let Some(svc) = st
                .authz
                .get_service_opt(service_name)
                .await
                .map_err(|e| CpError::BadRequest(format!("resolve ranger service: {e}")))?
            {
                let admins = admin_usernames(st).await?;
                st.authz
                    .seed_admin_all_access(svc.id, service_name, "starrocks", &admins)
                    .await
                    .map_err(|e| CpError::BadRequest(format!("seed admin policy: {e}")))?;
                recompile_after_infra_seed(st, svc.id).await;
            }
        }
    }

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

    // FE HTTP ingress, parsed from the stored top-level `spec["ingress"]` (create/
    // update persist it there). Only `ingress` is used off this Trino-shaped config
    // for a StarRocks engine — it's the field the provisioner's `starrocks_ingress`
    // renders from.
    let mut config = TrinoConfigReq {
        ingress: parse_ingress(&engine.spec["ingress"]),
        ..Default::default()
    };
    // Ingress TLS from a typed secret (FE HTTP ingress): resolve the referenced
    // kubernetes.io/tls secret, project it into `{name}-ingress-tls`, and point the
    // Ingress at it. Typed ref wins over a raw `tls_secret` string when both are set.
    if let Some(ing) = config.ingress.as_mut() {
        if let Some(ref_id) = ing.tls_cert_secret_ref {
            let (cert_pem, key_pem) = resolve_tls_cert_secret(st, ref_id).await?;
            ing.tls_cert_pem = Some(cert_pem);
            ing.tls_key_pem = Some(key_pem);
            ing.tls_secret = Some(format!("{}-ingress-tls", engine.name));
        }
    }

    // Last fallible step: a refused rotation (409) leaves nothing half-done.
    let ranger = ranger_binding(
        st,
        engine,
        ranger_service_name.clone(),
        rotate_ranger_credential,
    )
    .await?;
    let req = EngineProvisionReq {
        name: engine.name.clone(),
        namespace: engine.namespace.clone(),
        image_version: fe_image.clone(),
        config,
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
            fe_meta_storage_class,
            fe_meta_storage_size,
            fe_meta_persistent,
        }),
    };
    Ok((req, delegation_bootstrap))
}

/// Validate a StarRocks body's `delegationMode` (the SAME rule create and update
/// both apply, up front, before any spec is persisted): an omitted/blank mode is
/// fine (no delegation), otherwise it MUST be `"jwt"` or `"impersonate"`.
fn validate_sr_delegation_mode(sr_body: &StarRocksConfigBody) -> Result<(), CpError> {
    if let Some(m) = sr_body.delegation_mode.as_deref().filter(|s| !s.is_empty()) {
        if m != "jwt" && m != "impersonate" && m != "none" {
            return Err(CpError::BadRequest(format!(
                "invalid starrocks delegationMode: {m} (expected \"jwt\", \"impersonate\", or \"none\")"
            )));
        }
    }
    Ok(())
}

/// Reject a StarRocks body whose `feConfig` override tries to set a key egret
/// manages itself ([`egret_engine_mgmt_api::SR_PROTECTED_FE_KEYS`] — the
/// shared-data run mode, object-store coords, TLS keystore, and wire ports). The
/// SAME guard runs up front on both create and update, before any spec is
/// persisted, so a bad override 400s with nothing to roll back. The provisioner
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
/// `autoScalingPolicy` entirely, see the provisioner), so validation is SKIPPED
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

/// Reject an FE-metadata storage class or size that the kube-starrocks operator
/// would turn into "no PVC at all", or that the CRD would reject at apply time.
///
/// Both fields render INSIDE the FE StatefulSet's `volumeClaimTemplates`, so a
/// value the operator reads as "not a claim" does not merely mis-size the
/// volume — it removes the claim template, which on a live StatefulSet is an
/// immutable-field change that parks the `StarRocksCluster` at `phase=failed`
/// forever (see `validate_sr_fe_meta_immutable`). Three values do that:
///
/// - `storageClassName: "emptyDir"` and `"hostPath"` are SPECIAL to the
///   operator, not StorageClass names: it interprets them as "mount this kind
///   of volume instead" and skips the PVC entirely. Matched
///   case-INSENSITIVELY: being stricter than the operator can only refuse a
///   real StorageClass named `EmptyDir`, which is not a thing, whereas being
///   laxer would let a casing variant through if the operator ever normalizes.
/// - a ZERO size (`0`, `0Gi`, `0.0Gi`) makes the operator skip the claim too.
///
/// The size is also checked against the CRD's own quantity pattern
/// (`([0-9]*[.])?[0-9]+(Mi|Gi|Ti|Ei|Pi)`), so `"20"`, `"20GB"`, `"20Ki"` or
/// `"big"` are a 400 here instead of an apply-time CR rejection minutes later
/// with nothing readable to point at.
///
/// Runs on create AND update. On update the immutability guard below refuses
/// these fields outright, so this is the create-time gate in practice — but it
/// stays on both paths so a value that cannot be created cannot be reached by
/// an edit either (the rule `validate_sr_autoscaling` states).
fn validate_sr_fe_meta_storage(sr_body: &StarRocksConfigBody) -> Result<(), CpError> {
    if let Some(class) = sr_body
        .fe_meta_storage_class
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let lower = class.to_ascii_lowercase();
        if lower == "emptydir" || lower == "hostpath" {
            return Err(CpError::BadRequest(format!(
                "`starrocks.feMetaStorageClass: {class}` is not a StorageClass: the \
                 kube-starrocks operator treats `emptyDir` and `hostPath` as a request \
                 for an EPHEMERAL volume and renders no PersistentVolumeClaim, which \
                 defeats FE metadata persistence entirely. Omit the field to bind the \
                 cluster's default StorageClass, or name a real StorageClass"
            )));
        }
    }
    if let Some(size) = sr_body
        .fe_meta_storage_size
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if !fe_meta_size_is_a_positive_quantity(size) {
            return Err(CpError::BadRequest(format!(
                "`starrocks.feMetaStorageSize: {size}` is not a positive binary quantity: \
                 it must look like `20Gi` — digits with an optional decimal point and one \
                 of the Mi/Gi/Ti/Ei/Pi suffixes — and be greater than zero. A zero size \
                 makes the operator render no PersistentVolumeClaim at all, and any other \
                 shape is rejected by the StarRocksCluster CRD at apply time"
            )));
        }
    }
    Ok(())
}

/// `true` when `s` matches the CRD's quantity pattern for a binary size AND is
/// strictly greater than zero. Hand-rolled rather than a regex: the pattern is
/// small and this crate carries no regex dependency for it.
///
/// Accepts `20Gi`, `1.5Ti`, `.5Gi`. Rejects `0`, `0Gi`, `0.0Gi` (zero), `20`
/// (no suffix), `20Ki` / `20GB` / `20G` (not a CRD suffix), `1.2.3Gi`, `Gi`.
fn fe_meta_size_is_a_positive_quantity(s: &str) -> bool {
    const SUFFIXES: [&str; 5] = ["Mi", "Gi", "Ti", "Ei", "Pi"];
    let Some(suffix) = SUFFIXES.iter().find(|x| s.ends_with(**x)) else {
        return false;
    };
    let num = &s[..s.len() - suffix.len()];
    let mut dots = 0usize;
    let mut digits = 0usize;
    let mut nonzero = false;
    for c in num.chars() {
        if c == '.' {
            dots += 1;
            if dots > 1 {
                return false;
            }
        } else if c.is_ascii_digit() {
            digits += 1;
            if c != '0' {
                nonzero = true;
            }
        } else {
            return false;
        }
    }
    digits > 0 && nonzero
}

/// Refuse a PUT that tries to change the FE metadata volume of an engine that
/// already has one. `feMetaStorageClass` and `feMetaStorageSize` are CREATE-ONLY
/// for the same reason `feMetaPersistent` has no wire knob at all: both render
/// inside the FE StatefulSet's `spec.volumeClaimTemplates`, which Kubernetes
/// makes IMMUTABLE once the StatefulSet exists. Changing either does not resize
/// anything — the operator's merge patch is rejected by the API server, the
/// `StarRocksCluster` goes to `phase=failed`, and from then on NO change of any
/// kind reconciles for that engine, a Ranger rebind included. Measured on kind
/// 2026-09-26, §7 of `docs/en/plans/2026-09-26-fe-meta-live-result.md`.
///
/// Refusing rather than accepting-and-dropping follows the `catalogs`-on-update
/// rule in [`update_engine`]: silently ignoring the field would tell the caller
/// their volume was resized when it was not. There is deliberately NO bypass
/// flag — the only safe migration is outside egret, and the message says so.
///
/// Only a NON-BLANK body value is refused. Absent/blank is exactly the
/// omit=preserve case: it cannot change the render, so refusing it would break
/// an idempotent `GET` → `PUT` round-trip for no benefit.
///
/// An UNMARKED engine is refused too, with its own reason: its stored values are
/// never rendered (it keeps the operator's `emptyDir`), so accepting them would
/// persist a knob that silently does nothing.
fn validate_sr_fe_meta_immutable(
    sr_body: &StarRocksConfigBody,
    stored_spec: &serde_json::Value,
) -> Result<(), CpError> {
    let sent: Vec<&str> = [
        (
            "feMetaStorageClass",
            sr_body.fe_meta_storage_class.as_deref(),
        ),
        ("feMetaStorageSize", sr_body.fe_meta_storage_size.as_deref()),
    ]
    .into_iter()
    .filter(|(_, v)| v.map(str::trim).is_some_and(|s| !s.is_empty()))
    .map(|(k, _)| k)
    .collect();
    if sent.is_empty() {
        return Ok(());
    }
    let fields = sent
        .iter()
        .map(|f| format!("`starrocks.{f}`"))
        .collect::<Vec<_>>()
        .join(" and ");
    let marked = stored_spec
        .get("starrocksConfig")
        .and_then(|c| c.get("feMetaPersistent"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if marked {
        Err(CpError::BadRequest(format!(
            "{fields} is not accepted on update: it renders into the FE StatefulSet's \
             `volumeClaimTemplates`, which Kubernetes makes immutable once the \
             StatefulSet exists, so changing it does not resize the volume — every \
             operator reconcile is rejected and the StarRocksCluster stops converging \
             entirely. To change this engine's FE metadata volume, recreate its FE \
             StatefulSet yourself (`kubectl -n <namespace> delete statefulset \
             <engine>-fe --cascade=orphan`), which DESTROYS that FE's metadata \
             (catalogs, StarRocks-side users and grants), or create a new engine with \
             the class and size you want"
        )))
    } else {
        Err(CpError::BadRequest(format!(
            "{fields} is not accepted on update, and this engine has no FE metadata \
             PersistentVolumeClaim to configure: it was created before FE metadata \
             persistence shipped and keeps the operator's `emptyDir`, so the value \
             would be stored and never rendered. Create a new engine to get a \
             persistent FE metadata volume"
        )))
    }
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
/// default (`DEFAULT_ENGINE_IMAGES`, replicas `1`, resources
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
        .unwrap_or_else(|| DEFAULT_ENGINE_IMAGES.starrocks_fe_image.into());
    let cn_image = sr_body
        .cn_image
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| existing_str("cnImage"))
        .unwrap_or_else(|| DEFAULT_ENGINE_IMAGES.starrocks_cn_image.into());
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
    // FE metadata PVC class/size: same omit=preserve rule, and absent on both
    // sides stores `null`, which the provisioner reads as "default StorageClass"
    // + `DEFAULT_FE_META_STORAGE_SIZE`.
    //
    // Both are CREATE-ONLY in practice: `validate_sr_fe_meta_immutable` refuses
    // a non-blank value on update, because they render inside the FE
    // StatefulSet's immutable `volumeClaimTemplates`. The omit=preserve
    // fallbacks below therefore only ever carry a create-time value forward.
    //
    // An engine created BEFORE the PVC shipped gets no persistent meta volume
    // from these — it has no marker, so the renderer emits nothing at the meta
    // path and the operator's `emptyDir` default applies. That is deliberate:
    // adding the claim to a live StatefulSet wedges its whole cluster. Migrating
    // such an engine is a manual operator action, never an edit.
    //
    // The body is read ONLY on create (`existing.is_none()`). On update the
    // stored value is carried forward whatever the body says, so this builder
    // cannot change the FE metadata volume even if a future caller forgets
    // `validate_sr_fe_meta_immutable`: the refusal is what the operator SEES,
    // this is what makes the wedge unreachable. Two mechanisms on purpose —
    // a check the caller can skip is not a guarantee.
    let (fe_meta_storage_class, fe_meta_storage_size) = match existing {
        None => (
            sr_body
                .fe_meta_storage_class
                .clone()
                .filter(|s| !s.trim().is_empty())
                .map(serde_json::Value::from)
                .unwrap_or(serde_json::Value::Null),
            sr_body
                .fe_meta_storage_size
                .clone()
                .filter(|s| !s.trim().is_empty())
                .map(serde_json::Value::from)
                .unwrap_or(serde_json::Value::Null),
        ),
        Some(_) => (
            existing_cfg
                .and_then(|c| c.get("feMetaStorageClass"))
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            existing_cfg
                .and_then(|c| c.get("feMetaStorageSize"))
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        ),
    };
    // FE metadata PERSISTENCE MARKER — creation-time, and deliberately NOT a
    // field on `StarRocksConfigBody`. `existing_cfg.is_none()` is exactly
    // "this call is a create" (the only two production callers are
    // `create_starrocks_engine`, which passes `None`, and
    // `update_starrocks_engine`, which passes the stored spec), so a create
    // stamps `true` and every engine that already exists stays at `false`.
    //
    // No wire knob because `volumeClaimTemplates` is IMMUTABLE on a live
    // StatefulSet (measured on kind 2026-09-26, §7 of
    // `docs/en/plans/2026-09-26-fe-meta-live-result.md`): a PUT that could set
    // this to `true` would not migrate the engine, it would make every operator
    // reconcile rejected by the API server and park the `StarRocksCluster` at
    // `phase=failed` — no further change of any kind would apply to it,
    // including a Ranger rebind. There is no body value to honour, so an update
    // can only PRESERVE what is stored, which is the omit=preserve rule taken to
    // its limit.
    //
    // Matched on `existing`, NOT on `existing_cfg`: `existing_cfg` is `None`
    // both on create AND for a stored spec that carries no `starrocksConfig`
    // object at all, and conflating those would stamp the marker onto an
    // existing engine — the one outcome that must never happen.
    let fe_meta_persistent = match existing {
        None => serde_json::Value::Bool(true),
        Some(_) => existing_cfg
            .and_then(|c| c.get("feMetaPersistent"))
            .cloned()
            .unwrap_or(serde_json::Value::Bool(false)),
    };

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
            "feMetaStorageClass": fe_meta_storage_class,
            "feMetaStorageSize": fe_meta_storage_size,
            "feMetaPersistent": fe_meta_persistent,
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
/// bearer to expire). The attachment is recorded `pending` (D1); a background
/// step (spawned after `provision` succeeds, see
/// [`spawn_starrocks_catalog_wire`]) converges it — and every other attachment
/// of this engine — once the FE is Ready, and records what happened per row.
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
    extra_catalogs: Vec<egret_catalog_registry::CatalogRow>,
) -> Result<(StatusCode, Json<CreateEngineResponse>), CpError> {
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

    let mut sr_body = b.starrocks.as_ref().cloned().unwrap_or_default();

    // Delegation opt-in (Access parity slice 3, S3-T2b): validate the mode up
    // front, before any engine row exists, so a bad value 400s with nothing to
    // roll back. `None`/omitted = no delegation (slice-2 default, open engine).
    // TLS-on defaulting when delegation is configured is applied inside
    // `build_starrocks_spec` (mirrors the Trino path, which provisions TLS for
    // BOTH delegation modes — a StarRocks jwt login sends the caller's JWT in
    // cleartext via the mysql_clear_password plugin, so TLS matters here too and
    // disabling it must be an explicit opt-out).
    validate_sr_delegation_mode(&sr_body)?;

    // Default-on delegation (spec 2026-07-21): an omitted mode resolves to jwt
    // (trust egret's machine-IdP), degrading to impersonate when JWT can't be
    // guaranteed. Persist the RESOLVED concrete mode so the provision-req builder
    // and update-preserve path see an explicit value. Only create resolves the
    // default; update preserves the stored mode.
    let resolved = crate::delegation_default::resolve_delegation(
        sr_body.delegation_mode.as_deref(),
        crate::delegation_default::has_platform_jwks(&st.engine_jwks_uri, &st.engine_oidc_issuer),
        sr_body.fe_image.as_deref(),
    )
    .map_err(CpError::BadRequest)?;
    match resolved {
        crate::delegation_default::ResolvedDelegation::Jwt => {
            sr_body.delegation_mode = Some("jwt".into());
            // JWT is sent as a cleartext MySQL password → TLS is mandatory (the
            // connector fail-closes without it). Force it on regardless of the body.
            sr_body.tls_enabled = Some(true);
        }
        crate::delegation_default::ResolvedDelegation::Impersonate => {
            sr_body.delegation_mode = Some("impersonate".into());
        }
        crate::delegation_default::ResolvedDelegation::Open => {
            sr_body.delegation_mode = None; // no delegation block ⇒ open engine
        }
    }
    // Reject any `feConfig` override of an egret-managed fe.conf key up front.
    validate_sr_fe_config(&sr_body)?;
    // Reject an enabled autoscaling policy with out-of-range bounds up front, so
    // the admin gets a clean 400 instead of a deferred HPA-create failure.
    validate_sr_autoscaling(&sr_body)?;
    // Reject an FE-metadata class/size the operator would read as "no PVC"
    // (`emptyDir`, `hostPath`, a zero size) or the CRD would reject at apply
    // time, up front — this is the ONLY path that can set them, since update
    // refuses both fields outright.
    validate_sr_fe_meta_storage(&sr_body)?;

    // Kubernetes must be configured before anything is minted.
    let Some(prov) = st.engine_provisioner.clone() else {
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };

    // ── Iceberg external-catalog auto-wire (T6), step 1: mint a Catalog Client
    // for the warehouse — the EXACT same call the Trino path uses
    // (`ensure_engine_role` + `catalog.create_client`). Minted BEFORE the
    // engine row exists, so a mint failure here needs no rollback (nothing to
    // roll back yet), matching the Trino path's ordering. From here on, every
    // failure runs `compensate_failed_create`.
    let (wh_catalog, _) = st
        .engine_mgmt
        .registry()
        .ensure_warehouse_catalog_with_compiler(
            &WarehouseIdentity {
                id: wh.id,
                name: &wh.name,
                workspace_id: wh.workspace_id,
            },
            None,
            st.catalog_assignment_compiler.as_deref(),
        )
        .await?;
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
    let mut undo = CreateUndo {
        client_id: Some(client.id),
        ..CreateUndo::default()
    };
    match crate::catalog_apply::store_client_credential(&st, &client).await {
        Ok(id) => undo.credential_secret_id = Some(id),
        Err(e) => {
            compensate_failed_create(&st, undo).await;
            return Err(e);
        }
    }

    let namespace = DEFAULT_NAMESPACE.to_string();
    // Stored spec (identical builder create and update share). The delegation
    // block is persisted so `store::Engine::delegation_mode`/
    // `supports_jwt_delegation`/`supports_delegation` (read back on every
    // subsequent load, including at `EngineAssignmentService::assign` time) see
    // the SAME mode this create call configured. An omitted mode serializes to a
    // JSON `null`, which `store::Engine::delegation_mode`'s `.get("mode")` chain
    // treats exactly like a wholly-absent key — it must NOT default to
    // impersonate the way Trino's `trinoConfig.delegationMode` does.
    let mut spec = build_starrocks_spec(&sr_body, None);
    // Persist the FE HTTP ingress at TOP-LEVEL `spec["ingress"]` (kind-agnostic —
    // `build_starrocks_provision_req` reads it back into `config.ingress`, the
    // field the provisioner's `starrocks_ingress` renders from). The shared
    // create form builds `trinoConfig.ingress` even for a StarRocks engine, so the
    // wire ingress arrives on `b.trino_config`; copy it in when present.
    if let Some(ing) = b.trino_config.as_ref().and_then(|tc| tc.get("ingress")) {
        spec["ingress"] = ing.clone();
    }

    let engine = match st
        .engine_mgmt
        .create_engine(
            &b.name,
            "starrocks",
            &namespace,
            &spec,
            b.tls_cert_secret_ref,
        )
        .await
    {
        Ok(e) => e,
        Err(e) => {
            compensate_failed_create(&st, undo).await;
            return Err(e.into());
        }
    };
    undo.engine_id = Some(engine.id);

    // Step 2: attach the warehouse's catalog (the engine's PRIMARY catalog); its
    // credential secret is what the background FE wire presents.
    if let Err(e) = st
        .engine_mgmt
        .registry()
        .insert_attachment(&egret_catalog_registry::NewAttachment {
            engine_id: engine.id,
            catalog_id: wh_catalog.id,
            catalog_client_id: Some(client.id),
            credential_secret_id: undo.credential_secret_id,
            minted_client: true,
            applied_now: false,
            // The engine's PRIMARY catalog (SP3c D2), same as the Trino path.
            is_primary: true,
            created_at: None,
            // `pending`, not `applied` (D1, migration 0149): the FE is
            // unreachable for the five to ten minutes after `provision`, so a
            // row claiming `applied` here would be a claim about a cluster
            // nothing has spoken to yet. `spawn_starrocks_catalog_wire` settles
            // it once the FE answers, and a Retry converges it if that never
            // runs — which is what made this change safe to make only now
            // (Task 5), and not in Task 3's mechanical sweep.
            apply_status: egret_catalog_registry::store::apply_status::PENDING,
        })
        .await
    {
        compensate_failed_create(&st, undo).await;
        return Err(e.into());
    }

    // Optional Ranger bind to an EXISTING profile (validated the same way the
    // Trino auto-bind step validates `authzServiceId`).
    if let Some(sid) = b.authz_service_id {
        match st.authz.list_services().await {
            Ok(svcs) if svcs.iter().any(|s| s.id == sid) => {
                if let Err(e) =
                    crate::profile_compiler::bind_profile_guarded(&st, engine.id, &b.name, sid)
                        .await
                {
                    // Any refusal undoes the create: an engine created with an
                    // `authzServiceId` must never come out unbound.
                    compensate_failed_create(&st, undo).await;
                    return Err(e);
                }
            }
            Ok(_) => {
                compensate_failed_create(&st, undo).await;
                return Err(CpError::BadRequest(format!(
                    "authzServiceId {sid} is not a policy profile"
                )));
            }
            Err(e) => {
                tracing::error!(error = %e, "list_services failed during starrocks create");
                compensate_failed_create(&st, undo).await;
                return Err(CpError::Internal);
            }
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
        compensate_failed_create(&st, undo).await;
        return Err(e);
    }

    let engine = match st.engine_mgmt.get_engine(engine.id).await {
        Ok(Some(e)) => e,
        Ok(None) => {
            compensate_failed_create(&st, undo).await;
            return Err(CpError::NotFound);
        }
        Err(e) => {
            compensate_failed_create(&st, undo).await;
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
            compensate_failed_create(&st, undo).await;
            return Err(e);
        }
    };

    undo.cluster = Some((
        engine.r#type.clone(),
        engine.namespace.clone(),
        engine.name.clone(),
    ));
    if let Err(msg) = prov.provision(&req).await {
        compensate_failed_create(&st, undo).await;
        return Err(CpError::BadRequest(format!(
            "engine provisioning failed: {msg}"
        )));
    }
    retire_ranger_previous_after_apply(&st, engine.id, &req).await;

    // Step 3: best-effort, non-blocking post-provision wire. The FE takes
    // ~1-2 min to become Ready and (in dev) egret→FE reachability is a live
    // concern, so this is spawned rather than awaited — the create response
    // below returns as soon as `provision` above has kicked off, not after the
    // FE is actually queryable. A failure is logged, never surfaced to the
    // caller: the admin can retry by re-provisioning, or per attachment with
    // `POST /engines/{id}/catalogs/{cid}/apply` (D2), which converges exactly
    // one attachment — the primary included — and settles its row either way.
    // Present the minted FE `root` password (resolved into
    // `req.starrocks.root_password` by `build_starrocks_provision_req`) on the
    // post-Ready bootstrap connection; `None` for legacy engines keeps it
    // passwordless.
    spawn_starrocks_catalog_wire(
        st.clone(),
        engine.id,
        engine.name.clone(),
        engine.namespace.clone(),
        delegation_bootstrap,
    );

    // The body's non-primary catalogs. Same helper, same order as Trino: the
    // engine exists, so a failure here belongs to the catalog. A StarRocks FE is
    // unreachable for five to ten minutes after `provision`, which is exactly
    // when this runs, so `attach_to_engine` writes the row `pending`, fails to
    // reach the FE, and settles it to `failed` with the reason — an ATTACHMENT
    // that exists and is retryable, not an error, so it does NOT appear in
    // `catalogAttachErrors`. (Only a `CatalogOpError` does: a catalog this
    // engine cannot read, a name StarRocks reserves, or a database write that
    // failed.) The wire step above, or `POST …/catalogs/{cid}/apply`, converges
    // it once the FE answers.
    let catalog_attach_errors = attach_extra_catalogs(&st, &engine, &extra_catalogs).await;

    // Same as the Trino create's step 11: recompile the bound profile once
    // every catalog this engine reaches is attached.
    crate::profile_compiler::recompile_bound_profile(&st, engine.id, &engine.name).await;

    Ok((
        StatusCode::CREATED,
        Json(CreateEngineResponse {
            engine: engine.into(),
            catalog_attach_errors,
        }),
    ))
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
///
/// `pub(crate)` (SP3b Task 3): the connection-opts primitive
/// [`crate::starrocks_ddl::MysqlDdlRunner`] uses. Since Task 5 that runner is
/// the ONLY caller — the bootstrap's own `run_starrocks_ddl` is gone, and every
/// statement egret sends a StarRocks FE goes through the injectable seam.
pub(crate) fn starrocks_root_conn_opts(
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
/// All interpolated values are escaped with the registry's own
/// [`egret_catalog_registry::escape_dq`] — this runs as `root` on the FE, and
/// `jwks_url`/`issuer`/`audience` are admin-controlled config, not
/// charset-validated anywhere upstream. (SP3b Task 5: the duplicate
/// `sr_escape_dq`/`sr_escape_ident` this module carried are gone; the renderer
/// and these statements now escape through one implementation.)
fn starrocks_jwt_integration_sql(jwks_url: &str, issuer: &str, audience: &str) -> Vec<String> {
    let jwks_url = egret_catalog_registry::escape_dq(jwks_url);
    let issuer = egret_catalog_registry::escape_dq(issuer);
    let audience = egret_catalog_registry::escape_dq(audience);
    vec![
        format!(
            "CREATE SECURITY INTEGRATION egret_oidc PROPERTIES(\"type\"=\"authentication_jwt\", \
             \"jwks_url\"=\"{jwks_url}\", \"principal_field\"=\"preferred_username\", \
             \"required_issuer\"=\"{issuer}\", \"required_audience\"=\"{audience}\")"
        ),
        "ADMIN SET FRONTEND CONFIG (\"authentication_chain\"=\"native,egret_oidc\")".into(),
    ]
}

/// The repair half of [`starrocks_jwt_integration_sql`], for an FE that already
/// carries an `egret_oidc` integration whose properties have since drifted.
///
/// **Measured against StarRocks 3.x, not inferred** — the DDL surface here is
/// narrower than it looks and every alternative was tried on a live FE:
///
/// | statement | exists | missing |
/// |---|---|---|
/// | `CREATE` | error `already exists` | ok |
/// | `ALTER … SET` | ok, and the value really changes | error `not found` |
/// | `DROP` | ok | error `not found` |
/// | `CREATE IF NOT EXISTS` | **syntax error** | **syntax error** |
/// | `DROP IF EXISTS` | **syntax error** | **syntax error** |
///
/// So there is no single idempotent statement to reach for: `IF [NOT] EXISTS`
/// is not part of the grammar for this object. `CREATE`-then-`ALTER`-on-exists
/// is the only shape that converges from either starting state.
///
/// This matters because the properties DO drift. `required_issuer` is egret's
/// own machine-IdP issuer, and changing it — as #528 did — leaves every engine
/// provisioned earlier rejecting freshly minted tokens, with nothing in the
/// system repairing it: the bootstrap runs only at engine-create time.
///
/// `type` is deliberately not re-sent: it is the integration's kind, not a
/// setting, and StarRocks has no reason to accept it changing underneath.
/// Escaping is [`egret_catalog_registry::escape_dq`], identical to the
/// `CREATE` above — this runs as `root` on the FE from the same
/// admin-controlled config.
fn starrocks_jwt_integration_repair_sql(jwks_url: &str, issuer: &str, audience: &str) -> String {
    let jwks_url = egret_catalog_registry::escape_dq(jwks_url);
    let issuer = egret_catalog_registry::escape_dq(issuer);
    let audience = egret_catalog_registry::escape_dq(audience);
    format!(
        "ALTER SECURITY INTEGRATION egret_oidc SET (\"jwks_url\"=\"{jwks_url}\", \
         \"principal_field\"=\"preferred_username\", \"required_issuer\"=\"{issuer}\", \
         \"required_audience\"=\"{audience}\")"
    )
}

/// Build the `CREATE USER IF NOT EXISTS` DDL for the impersonation service
/// user (the connection's basic-auth login on the impersonate path — S3-T1's
/// `StarRocksConnector` logs in as this user, then runs `EXECUTE AS
/// <session_user>`). `IDENTIFIED BY` takes a single-quoted string literal
/// (StarRocks/MySQL `CREATE USER` grammar), so both the username and the
/// password go through [`egret_query_api::quote_string`] — which supplies the
/// surrounding quotes and escapes **both** `'` (doubled) and `\`. StarRocks is
/// MySQL-dialect, so a backslash escapes the character after it: a value
/// containing `\'` closes a quote-only-escaped literal early and injects DDL
/// that runs as `root` on the FE. Neither input is a constant — the password is
/// read back from a mutable stored secret (`ensure_delegator_secret`) on every
/// call after the first.
fn starrocks_create_service_user_sql(service_user: &str, password: &str) -> String {
    use egret_query_api::{quote_string, Engine as QueryEngine};
    format!(
        "CREATE USER IF NOT EXISTS {}@'%' IDENTIFIED BY {}",
        quote_string(QueryEngine::StarRocks, service_user),
        quote_string(QueryEngine::StarRocks, password)
    )
}

/// Build the `GRANT IMPERSONATE` DDL authorizing the service user to run
/// `EXECUTE AS <any user> WITH NO REVERT` (the impersonate-mode primitive —
/// see `crate::execution::connector::starrocks::StarRocksConnector::switch_identity`
/// in `egret-query`). Backticks in the identifier are doubled (MySQL/StarRocks
/// identifier-escaping rule) by [`egret_catalog_registry::escape_ident`] —
/// the function itself since fix round 1 (F6), not a local copy of it under a
/// comment claiming to match its "convention", which a future edit could
/// falsify without anything noticing.
fn starrocks_impersonation_grant_sql(service_user: &str) -> String {
    format!(
        "GRANT IMPERSONATE ON ALL USERS TO `{}`",
        egret_catalog_registry::escape_ident(service_user)
    )
}

/// Runtime bundle for the post-Ready delegation bootstrap (S3-T2 step 3,
/// wired up in S3-T2b): the JWT security integration + the impersonation
/// service user. `None` on [`spawn_starrocks_catalog_wire`]'s `delegation`
/// param skips the bootstrap entirely — [`create_starrocks_engine`] passes
/// `None` when the body's `delegationMode` is omitted (no delegation opted
/// into) and `Some(..)` for either delegation mode, built from
/// `st.engine_jwks_uri`/`engine_oidc_issuer` plus a minted
/// [`crate::delegator_policy::DELEGATOR_USERNAME`] secret.
///
/// SP3b Task 5 removed this bundle's `tls_cert_pem`: the certificate is now
/// read off the engine row by [`crate::starrocks_ddl::fe_target`], which every
/// statement of the bootstrap shares, so there is no second place to forget it
/// (D6). The two were always the same value — `ensure_tls_cert` persists the
/// cert with `set_tls` before handing it back to the provisioning request.
#[derive(Clone)]
struct StarRocksDelegationBootstrap {
    jwks_url: String,
    issuer: String,
    audience: String,
    service_user: String,
    service_password: String,
}

/// Whether a background StarRocks bootstrap may still write to the engine it
/// was spawned for: the row must still exist under the same id and name and
/// not be on its way out (`DELETING`) or already given up on (`FAILED` —
/// Task 12's compensation may not have been able to remove it, and its
/// cluster objects may already be gone). A delete, a delete plus same-name
/// re-create (new id), or a status flip to `DELETING`/`FAILED` all stop the
/// bootstrap instead of running DDL against a cluster it no longer owns.
fn bootstrap_target_is_current(found: Option<&Engine>, engine_name: &str) -> bool {
    matches!(found, Some(e) if e.name == engine_name
        && e.status != crate::engine_status::DELETING
        && e.status != crate::engine_status::FAILED)
}

/// How many consecutive `get_engine` read failures [`engine_still_current`]
/// tolerates before it gives up on the bootstrap. A single blip must not
/// permanently abandon a healthy bootstrap that checks in up to ~60 times
/// over its ~10-minute budget (30 FE-ready polls + 30 root-login polls, each
/// a checkpoint) — but a store that is genuinely unreachable must not spin
/// the task forever either. 5 is chosen so a handful of back-to-back
/// failures (more than one bad connection acquisition in a row, fewer than a
/// whole poll cycle) is what it takes to conclude the store, not the
/// network, is the problem.
pub(crate) const MAX_CONSECUTIVE_ENGINE_READ_ERRORS: u32 = 5;

/// Re-read the engine and apply [`bootstrap_target_is_current`]. A DEFINITE
/// answer — the row is gone, renamed, deleting or failed — stops the
/// bootstrap immediately. A `get_engine` READ ERROR does NOT: it is
/// tolerated (logged at `warn`, this checkpoint treated as "still current")
/// like the existing tolerant cluster-poll arms elsewhere in
/// `spawn_starrocks_catalog_wire`, unless it has now failed
/// [`MAX_CONSECUTIVE_ENGINE_READ_ERRORS`] times in a row, at which point it
/// also stops — an endless loop against a truly dead store must still end.
/// `consecutive_read_errors` is owned by the caller: one counter per spawned
/// bootstrap, threaded through every checkpoint, so only BACK-TO-BACK
/// failures count — any successful read (whatever verdict it carries)
/// resets it to 0.
pub(crate) async fn engine_still_current(
    store: &crate::EngineMgmtStore,
    engine_id: Uuid,
    engine_name: &str,
    consecutive_read_errors: &mut u32,
) -> bool {
    let current = match store.get_engine(engine_id).await {
        Ok(found) => {
            *consecutive_read_errors = 0;
            bootstrap_target_is_current(found.as_ref(), engine_name)
        }
        Err(e) => {
            *consecutive_read_errors += 1;
            if *consecutive_read_errors >= MAX_CONSECUTIVE_ENGINE_READ_ERRORS {
                tracing::warn!(engine = %engine_name, error = %e,
                    consecutive_errors = *consecutive_read_errors,
                    "starrocks bootstrap: could not re-read the engine after \
                     several consecutive attempts; stopping");
                false
            } else {
                tracing::warn!(engine = %engine_name, error = %e,
                    consecutive_errors = *consecutive_read_errors,
                    "starrocks bootstrap: could not re-read the engine; \
                     tolerating and retrying at the next checkpoint");
                true
            }
        }
    };
    if !current {
        tracing::info!(engine = %engine_name, %engine_id,
            "starrocks bootstrap: engine deleted, re-created, failed, deleting, \
             or unreachable too many times in a row; stopping");
    }
    current
}

/// How many times the two post-provision poll loops (FE readiness, then a real
/// `root` login) ask before giving up: 30 × [`POLL_INTERVAL`] ≈ 5 minutes each.
///
/// It is a constant the TEST BUILD SHRINKS rather than a literal in the loop
/// because the unhappy paths — an FE that never reports ready, a root login
/// that never succeeds — are exactly what D8's behavioural tests drive, and at
/// the production budget each such case would cost five minutes of real
/// sleeping. The trade that buys: `MAX_POLLS` itself is not under test, and a
/// happy-path test must make the FIRST poll succeed. What these tests are for
/// is the checkpoints and the loop, not arithmetic on a constant.
#[cfg(not(test))]
const MAX_POLLS: u32 = 30;
#[cfg(test)]
const MAX_POLLS: u32 = 1;
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

/// Best-effort, non-blocking post-provision convergence: poll until an FE pod
/// is ready, wait for a real `root` login (the `initpwd` Job races the FE),
/// then bring EVERY attachment of this engine onto the FE and record the
/// outcome of each one (design D2) — where before it created exactly one
/// catalog, with `CREATE EXTERNAL CATALOG IF NOT EXISTS` (which silently adopts
/// whatever is already there, the hole D3 exists to close), and only logged
/// what happened. When `delegation` is `Some`, the JWT integration and
/// impersonation service user follow, as before.
///
/// It takes the whole [`CpState`] rather than twelve positional arguments
/// because it now reads the registry, the secrets and the storage registry
/// itself; `CpState` is `Clone` and `'static`, so the spawned task owns a
/// clone. The provisioner comes from `st.engine_provisioner` — a `None` there
/// ends the task with a warning, as every other path treats it.
///
/// Every statement — the root-login probe included — goes through
/// `st.starrocks_ddl` with the one [`crate::starrocks_ddl::FeTarget`] this
/// engine resolves to, so the FE root password and every catalog credential
/// carry the engine's pinned certificate instead of crossing a TLS-capable
/// wire in cleartext (D6), and an FE error reaches a log or a column only
/// through [`crate::starrocks_ddl::FeError::scrubbed`].
///
/// Spawned via `tokio::spawn` so `create_starrocks_engine` returns its HTTP
/// response without waiting on any of this — the FE takes ~1-2 min to come up,
/// and neither the poll budget nor FE/egret reachability in a given cluster is
/// something the create request should block on. Returns the spawned task's
/// `JoinHandle` purely so a test can `.await` it for a deterministic "the
/// bootstrap has finished" signal instead of a guessed sleep; both production
/// call sites ignore it.
///
/// **Two call sites, two starting states** (final whole-branch review, F5).
/// [`create_starrocks_engine`] reaches this with every attachment `pending`;
/// Task 5 also reaches it from [`update_starrocks_engine`], where the engine's
/// attachments are already `applied`. So the give-up paths below say the
/// attachments keep the status they already have — they must not claim
/// `pending`, which is only the create path's starting state.
///
/// **Where the six `engine_still_current` checkpoints sit, and why.** They
/// exist so a bootstrap cannot resurrect an engine that was deleted or renamed
/// while it polled, and a loop over N attachments must not weaken that. One per
/// iteration of each poll loop (1, 2); one per ATTACHMENT inside
/// [`crate::catalog_apply::converge_starrocks_catalogs`] (3) — where before it
/// guarded the single catalog statement, so a delete part-way through a
/// ten-catalog engine now stops at the next catalog instead of finishing the
/// batch; then one before the security integration (4), one before its repair
/// (5), and one per remaining delegation statement (6). The rule is the same
/// everywhere: re-read immediately before a WRITE to the cluster, never once
/// for a batch of them.
fn spawn_starrocks_catalog_wire(
    st: CpState,
    engine_id: Uuid,
    engine_name: String,
    namespace: String,
    delegation: Option<StarRocksDelegationBootstrap>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let Some(prov) = st.engine_provisioner.clone() else {
            tracing::warn!(engine = %engine_name,
                "starrocks bootstrap: kubernetes is not configured; nothing to poll");
            return;
        };
        // One counter for the whole bootstrap, threaded through every
        // `engine_still_current` checkpoint — including the per-attachment one
        // inside `converge_starrocks_catalogs`, which is why that function
        // borrows it rather than starting a fresh count: see
        // `MAX_CONSECUTIVE_ENGINE_READ_ERRORS`.
        let mut consecutive_read_errors: u32 = 0;
        let mut fe_ready = false;
        for poll in 0..MAX_POLLS {
            if !engine_still_current(
                &st.engine_mgmt,
                engine_id,
                &engine_name,
                &mut consecutive_read_errors,
            )
            .await
            {
                return;
            }
            match prov.status("starrocks", &namespace, &engine_name).await {
                Ok(s) => {
                    if s.pods.iter().any(|p| p.name.contains("-fe-") && p.ready) {
                        fe_ready = true;
                        break;
                    }
                }
                Err(e) => {
                    tracing::debug!(engine = %engine_name, error = %e,
                        "starrocks bootstrap: status poll failed");
                }
            }
            // No sleep after the LAST attempt: the budget is exhausted either
            // way, and waiting one more interval only delays the warning.
            if poll + 1 < MAX_POLLS {
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }
        if !fe_ready {
            tracing::warn!(engine = %engine_name,
                "starrocks bootstrap: FE never reported ready within the poll budget; \
                 skipping — every attachment keeps the status it already has and a \
                 Retry converges it");
            return;
        }

        // The engine row itself, for the FE connection (host, port, root
        // password, pinned certificate) and for the attachments to converge.
        // A plain read, not a seventh checkpoint: the verdict this read cannot
        // reach — a rename, a `deleting`/`failed` flip — is taken by the root
        // login loop's own checkpoint immediately below, before anything is
        // written.
        let engine = match st.engine_mgmt.get_engine(engine_id).await {
            Ok(Some(e)) => e,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(engine = %engine_name, error = %e,
                    "starrocks bootstrap: could not re-read the engine; stopping");
                return;
            }
        };
        let fe = match crate::starrocks_ddl::fe_target(&st, &engine).await {
            Ok(fe) => fe,
            Err(e) => {
                tracing::warn!(engine = %engine_name, error = %e,
                    "starrocks bootstrap: could not resolve the FE connection; nothing applied");
                return;
            }
        };

        // (Everything on this connection logs in as `root`, so the root
        // password is a secret EVERY error below could quote — a failed login
        // echoes the credential it was given. This used to be a `fe_secrets`
        // map assembled here and threaded through each `scrubbed` call by
        // hand, which is the thing that was forgotten twice; `scrubbed` now
        // takes `&fe` and derives it. Final whole-branch review, F2.)

        // FE-pod-Ready is NOT root-login-ready: the `{name}-initpwd` Job (#268)
        // sets root's password via a SEPARATE Job that races the FE coming up, so
        // the pod reports Ready before the password is set. Running the bootstrap
        // DDL now would hit `Access denied for user 'root' (using password: YES)`.
        // Probe a real root login (a trivial `SELECT 1`) until it succeeds, so the
        // DDL below runs only once initpwd has won. Bounded like the pod poll.
        let mut auth_ready = false;
        for poll in 0..MAX_POLLS {
            if !engine_still_current(
                &st.engine_mgmt,
                engine_id,
                &engine_name,
                &mut consecutive_read_errors,
            )
            .await
            {
                return;
            }
            match st.starrocks_ddl.query(&fe, "SELECT 1").await {
                Ok(_) => {
                    auth_ready = true;
                    break;
                }
                Err(e) => {
                    // The probe carries the root password and nothing else, so
                    // what comes back can quote it: `scrubbed` is the only way
                    // to read the diagnosis, and the `&fe` it is given is what
                    // closes the module's documented short-unquoted residual
                    // for that value.
                    tracing::debug!(engine = %engine_name,
                        error = %e.scrubbed(&fe, &std::collections::BTreeMap::new()),
                        "starrocks bootstrap: root login not ready yet (initpwd race); retrying");
                }
            }
            if poll + 1 < MAX_POLLS {
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }
        if !auth_ready {
            tracing::warn!(engine = %engine_name,
                "starrocks bootstrap: root login never succeeded within the poll budget \
                 (initpwd race?); skipping — every attachment keeps the status it \
                 already has");
            return;
        }

        // Checkpoint 3 lives inside this loop, once per attachment.
        crate::catalog_apply::converge_starrocks_catalogs(
            &st,
            &engine,
            &fe,
            &mut consecutive_read_errors,
        )
        .await;

        // Delegation bootstrap (S3-T2 step 3): JWT integration + impersonation
        // service user, best-effort + logged like the catalog convergence above.
        if let Some(d) = delegation {
            let mut stmts = starrocks_jwt_integration_sql(&d.jwks_url, &d.issuer, &d.audience);
            stmts.push(starrocks_create_service_user_sql(
                &d.service_user,
                &d.service_password,
            ));
            stmts.push(starrocks_impersonation_grant_sql(&d.service_user));
            let mut all_ok = true;
            // The service user's password is inline in `CREATE USER … IDENTIFIED
            // BY` (D6), so every FE error below is scrubbed against it before it
            // reaches a log. That is the STATEMENT's half and the only half a
            // caller supplies; the root password this connection logged in
            // with — on the wire for these statements exactly as it was for
            // the probe — comes from the `&fe` each `scrubbed` call is handed.
            // The statements themselves are never logged.
            let mut secrets = std::collections::BTreeMap::new();
            secrets.insert(Uuid::from_u128(1), d.service_password.clone());

            // The integration's `CREATE` is the one statement that is NOT
            // idempotent, and it is the one whose inputs drift: `required_issuer`
            // is egret's own machine-IdP issuer. Re-running the bootstrap against
            // an FE that already has `egret_oidc` fails with `already exists` and
            // leaves the stale issuer in place — so this engine goes on rejecting
            // tokens egret is currently minting, and nothing in the system says so.
            //
            // StarRocks has no `CREATE … IF NOT EXISTS` for this object (measured:
            // it is a syntax error, as is `DROP … IF EXISTS`), so converging from
            // either starting state takes two statements, not one.
            if !engine_still_current(
                &st.engine_mgmt,
                engine_id,
                &engine_name,
                &mut consecutive_read_errors,
            )
            .await
            {
                return;
            }
            let create_integration = stmts.remove(0);
            match st.starrocks_ddl.execute(&fe, &create_integration).await {
                Ok(()) => {}
                // `is_already_exists`, NOT a `contains` on `scrubbed` (fix
                // round 1, F2): the scrubbed string is capped and, when the
                // fail-closed check fires, replaced entirely — so a message
                // carrying a SQL-escaped apostrophe would answer `false` here
                // and leave a stale `required_issuer` in place for ever.
                Err(e) if e.is_already_exists() => {
                    if !engine_still_current(
                        &st.engine_mgmt,
                        engine_id,
                        &engine_name,
                        &mut consecutive_read_errors,
                    )
                    .await
                    {
                        return;
                    }
                    let repair =
                        starrocks_jwt_integration_repair_sql(&d.jwks_url, &d.issuer, &d.audience);
                    match st.starrocks_ddl.execute(&fe, &repair).await {
                        Ok(()) => tracing::info!(engine = %engine_name,
                            "starrocks delegation bootstrap: egret_oidc already existed; \
                             properties re-applied so a changed issuer takes effect"),
                        Err(e) => {
                            all_ok = false;
                            tracing::warn!(engine = %engine_name, error = %e.scrubbed(&fe, &secrets),
                                "starrocks delegation bootstrap: egret_oidc exists but could not \
                                 be updated; JWT logins will fail if the issuer has changed");
                        }
                    }
                }
                Err(e) => {
                    all_ok = false;
                    tracing::warn!(engine = %engine_name, error = %e.scrubbed(&fe, &secrets),
                        "starrocks delegation bootstrap: statement failed; admin can retry");
                }
            }

            for stmt in &stmts {
                if !engine_still_current(
                    &st.engine_mgmt,
                    engine_id,
                    &engine_name,
                    &mut consecutive_read_errors,
                )
                .await
                {
                    return;
                }
                if let Err(e) = st.starrocks_ddl.execute(&fe, stmt).await {
                    all_ok = false;
                    tracing::warn!(engine = %engine_name, error = %e.scrubbed(&fe, &secrets),
                        "starrocks delegation bootstrap: statement failed; admin can retry");
                }
            }
            if all_ok {
                tracing::info!(engine = %engine_name,
                    "starrocks delegation bootstrap: JWT integration + impersonation user wired");
            }
        }
    })
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
    let mut engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    // A `deleting` row is the retry state of an unfinished delete; the live
    // status of a half-deleted cluster would hide it, so it wins.
    let status = if crate::external::is_external(&engine) {
        // Never ask the provisioner: there is no cluster of ours to ask about,
        // and a `not found` from it would read as an outage.
        EngineStatusDto {
            phase: crate::external::ORIGIN_EXTERNAL.into(),
            message: Some("External: egret does not provision or monitor this engine".into()),
            pods: vec![],
        }
    } else if engine.status == crate::engine_status::DELETING {
        EngineStatusDto {
            phase: engine.status.clone(),
            message: Some("a previous delete did not finish; retry the delete".into()),
            pods: vec![],
        }
    } else {
        match st.engine_provisioner.as_ref() {
            Some(prov) => match prov
                .status(&engine.r#type, &engine.namespace, &engine.name)
                .await
            {
                Ok(s) => {
                    // Record the first all-pods-ready observation. Conditional on the
                    // row still being `provisioning`, so `deleting`/`failed` stay put.
                    if s.phase == crate::engine_status::RUNNING
                        && engine.status == crate::engine_status::PROVISIONING
                    {
                        match st
                            .engine_mgmt
                            .transition_engine_status(
                                engine.id,
                                crate::engine_status::PROVISIONING,
                                crate::engine_status::RUNNING,
                            )
                            .await
                        {
                            Ok(true) => engine.status = crate::engine_status::RUNNING.to_string(),
                            Ok(false) => {}
                            Err(e) => tracing::warn!(engine = %engine.name, error = %e,
                                "could not record running status"),
                        }
                    }
                    EngineStatusDto {
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
                    }
                }
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
        }
    };
    Ok(Json(EngineDetailDto {
        engine: engine.into(),
        status,
    }))
}

/// Carry the stored Trino `delegationMode` into a new spec whose `trinoConfig`
/// omits it. An absent key reads back as `impersonate`
/// (`store::Engine::delegation_mode`), so dropping it would silently change the
/// engine's delegation mode.
fn preserve_trino_delegation_mode(spec: &mut serde_json::Value, stored: &serde_json::Value) {
    let Some(mode) = stored
        .get("trinoConfig")
        .and_then(|t| t.get("delegationMode"))
        .cloned()
    else {
        return;
    };
    if let Some(tc) = spec.get_mut("trinoConfig").and_then(|t| t.as_object_mut()) {
        tc.entry("delegationMode").or_insert(mode);
    }
}

/// Put back the spec an update persisted before its build or apply failed.
/// `update_engine` never writes `status` (see its doc comment), so there is no
/// status to restore — only the spec. Logged, not returned: the caller is
/// already returning that failure.
///
/// The restore is gated the same way the original write was:
/// `expect_status` is the status observed right before that write (and so
/// still the row's status if nothing else has touched it since). If the row
/// has since moved to a different status — typically a concurrent delete —
/// the restore matches zero rows and is a no-op; that is logged as an error
/// (not a warning), since the caller's 502 and its OpenAPI description both
/// promise the previous spec was restored, and here it was not.
///
/// This restores the STORED SPEC only. A pair of internal secrets `apply_built`
/// may have committed on the failed apply attempt (Task 7's
/// `commit_internal_secrets`, called unconditionally on both the success and
/// failure path) is NOT rolled back here, by design: the cluster's live
/// Secret/ConfigMap may already carry the fresh pair even though the apply as a
/// whole failed on a later object, and deleting the committed pair in that case
/// would strand the running pods on a value no longer on record. See
/// `apply_built`'s doc comment for the full argument.
async fn restore_engine_spec(
    st: &CpState,
    id: Uuid,
    spec: &serde_json::Value,
    expect_status: &str,
) {
    match st.engine_mgmt.update_engine(id, spec, expect_status).await {
        Ok(true) => {}
        Ok(false) => {
            tracing::error!(
                engine_id = %id,
                expect_status,
                "could not restore the engine spec after a failed update: the row's status \
                 changed before the restore could run (likely a concurrent delete); \
                 the stored spec is ahead of the cluster"
            );
        }
        Err(e) => {
            tracing::error!(engine_id = %id, expect_status, error = %e,
                "could not restore the engine spec after a failed update; \
                 the stored spec is ahead of the cluster");
        }
    }
}

/// Re-materialize every workspace assigned to `engine_id`, shared by both
/// update paths so they can't drift on the mapping. The CR was already
/// applied by the time this runs, so a failure here must surface loudly
/// (400) rather than silently leave stale connection options behind.
async fn rematerialize_or_400(st: &CpState, engine_id: Uuid) -> Result<(), CpError> {
    st.assignment
        .rematerialize_for_engine(engine_id)
        .await
        .map_err(|e| {
            CpError::BadRequest(format!(
                "engine updated but re-materializing workspace connections failed: {e}"
            ))
        })
}

/// Put `engine_id`'s policy-profile binding back to `previous` after a PUT that
/// bound successfully but then failed further down.
///
/// **Why a compensation and not just ordering.** Hoisting the cheap guards
/// above the bind (see [`update_engine`]) removes every refusal that can be
/// decided from the body and the stored row alone. It cannot remove the ones
/// derived from the BUILT spec — a missing primary warehouse, resource-group
/// validation, an unresolvable TLS or image-pull secret, the provisioner
/// itself. Those still 400/409/502 after the bind has committed, and a caller
/// told "no" must not find their engine re-pointed anyway. So the binding is
/// restored, the same way `restore_engine_spec` restores the spec.
///
/// Best-effort and loud: every failure logs at `error`, because the caller has
/// already been told the update did not happen. Never deletes a profile row —
/// `previous` may be `None` (the engine was unbound), which detaches, and
/// detaching is not deletion (standing constraint).
async fn restore_engine_authz_service(st: &CpState, engine_id: Uuid, previous: Option<i64>) {
    if let Err(e) = st
        .engine_mgmt
        .set_engine_authz_service(engine_id, previous)
        .await
    {
        tracing::error!(engine_id = %engine_id, previous = ?previous, error = %e,
            "could not restore the engine's policy-profile binding after a failed update; \
             the engine is bound to a profile the caller was told was not applied");
        return;
    }
    // The profile the engine is back on must be current again: the failed
    // attempt compiled the OTHER one, and nothing has recompiled this one
    // since. Fire-and-forget, like every other post-hoc recompile on this
    // path — the binding is already correct, and a stale compile is visible in
    // the logs with a repair URL.
    if let (Some(sid), Some(compiler)) = (previous, st.perm_compiler.clone()) {
        if let Err(e) = compiler.recompile_profile(sid).await {
            tracing::error!(engine_id = %engine_id, service_id = sid, error = %e,
                "restored the engine's previous policy profile but could not recompile it; \
                 it will serve a stale policy set until POST \
                 /api/control-plane/perm/compile/{sid} succeeds");
        }
    }
}

/// Recompile the profile an engine has just LEFT (engine grants spec C11/E10).
/// An engine grant compiles into its engine's profile; the bind path compiles
/// only the profile the engine moves to, and the delete paths compiled none,
/// so a former profile kept serving the engine's `perm-engine-` policies.
///
/// After commit, fire-and-forget and logged with the repair URL, as
/// [`restore_engine_authz_service`] does: not in the binding's transaction,
/// because the port recompiles one profile per call under its own
/// `lock_services_in_tx`, and two calls in one transaction would not take both
/// profile locks in one ascending order. `only_if_shared` (the delete paths):
/// with no engine left on the profile nothing downloads it, and the next bind
/// recompiles it, so only a profile another engine still binds is recompiled.
async fn recompile_former_profile(st: &CpState, previous: Option<i64>, only_if_shared: bool) {
    let (Some(sid), Some(compiler)) = (previous, st.perm_compiler.clone()) else {
        return;
    };
    if only_if_shared {
        match st
            .engine_mgmt
            .count_engines_bound_to_authz_service(sid)
            .await
        {
            Ok(0) => return,
            Ok(_) => {}
            Err(e) => {
                tracing::error!(service_id = sid, error = %e,
                    "could not tell whether an engine's former policy profile is still bound; \
                     it may serve a stale policy set until POST \
                     /api/control-plane/perm/compile/{sid} succeeds");
                return;
            }
        }
    }
    if let Err(e) = compiler.recompile_profile(sid).await {
        tracing::error!(service_id = sid, error = %e,
            "could not recompile the policy profile an engine left; it will serve a stale \
             policy set until POST /api/control-plane/perm/compile/{sid} succeeds");
    }
}

/// Every refusal a profile bind makes BEFORE it writes anything, shared by
/// `PUT /engines/{id}` ([`apply_authz_service_update`]) and external-engine
/// registration so the two cannot drift: the profile must exist (400), its
/// type must equal the engine's (409), no other engine may bind it (409), and
/// a Trino profile needs a usable `tagService` (400). `engine_type` and
/// `engine_name` describe the engine the profile is for, which for a
/// registration does not exist yet.
pub(crate) async fn check_profile_bindable(
    st: &CpState,
    sid: i64,
    engine_type: &str,
    engine_name: &str,
) -> Result<(), CpError> {
    let services = st
        .authz
        .list_services()
        .await
        .map_err(|e| CpError::BadRequest(format!("list policy profiles: {e}")))?;
    let Some(svc) = services.into_iter().find(|s| s.id == sid) else {
        return Err(CpError::BadRequest(format!(
            "authz_service_id {sid} is not a policy profile; \
             list them with GET /api/control-plane/policy/services"
        )));
    };
    // I1: the profile's TYPE must match the engine's kind, the same 409 the
    // create path already raises (`plan_trino_profile`). A cross-kind bind is
    // a fail-open, not a cosmetic mismatch: the plugin downloads the
    // service-def of the profile's own type, so a StarRocks engine bound to a
    // `trino` profile evaluates trino-shaped policy definitions against
    // starrocks-shaped compiled policies — and resource names that no longer
    // match are not denials, they are absent rules, which is an unmasked read.
    if svc.r#type != engine_type {
        return Err(CpError::Conflict(format!(
            "policy profile {sid} ({}) has type {}, but engine {} is a {} engine; \
             bind a profile of the same type",
            svc.name, svc.r#type, engine_name, engine_type
        )));
    }
    // I2: refuse a profile another engine already binds — again mirroring
    // create (`plan_trino_profile`'s `count_engines_bound_to_authz_service`
    // arm). Two engines sharing one profile silently share one policy set, so
    // a grant written for one governs the other. This engine cannot be the
    // one counted: the `== Some(sid)` early-return above already handled
    // "already bound here", so any bound engine at this point is a different
    // one.
    let bound_elsewhere = st
        .engine_mgmt
        .count_engines_bound_to_authz_service(sid)
        .await?;
    if bound_elsewhere > 0 {
        return Err(CpError::Conflict(format!(
            "policy profile {sid} ({}) is already bound to {bound_elsewhere} other engine(s); \
             one profile per engine",
            svc.name
        )));
    }
    // I3, fail-open: a TRINO profile with no USABLE `tagService` compiles zero
    // tag policies, so every tag-selected mask and row filter silently
    // disappears and the columns they protect are served raw —
    // `recompile_service_in_tx` records each one in `CompileSummary::skipped`
    // and carries on, by design (aborting there would stop every OTHER profile
    // getting the mask too).
    //
    // DECIDED: REFUSE, rather than bind-and-warn. Three reasons.
    //   1. It is the only moment a human is present. Nothing is provisioned
    //      yet and the engine keeps its previous, working binding, so the cost
    //      of being wrong is one rejected API call; the cost of proceeding is
    //      unmasked columns that nothing will ever complain about.
    //   2. "No tag rules exist right now, so a tagless profile is harmless" is
    //      a time-of-check/time-of-use trap. The binding persists; the `perm`
    //      model does not stand still, and a Trino tag rule is catalog-
    //      UNSCOPED, so the next tag rule anyone writes anywhere would
    //      silently fail to reach this engine. There would be nobody watching.
    //   3. It blocks nothing legitimate. Auto-bind always creates `{name}_tag`
    //      and wires `tagService`, so every Trino profile egret itself makes
    //      passes this check; one that fails is hand-made or damaged.
    //
    // **TRINO ONLY, and that is not an oversight.** StarRocks' Ranger plugin
    // has no tag policies at all (upstream starrocks#67458 — see
    // `egret_policy::service_def::STARROCKS_SERVICE_DEF_JSON`), so a StarRocks
    // profile is not supposed to have a tag service: the compiler EXPANDS a
    // tag-selector rule into per-column resource policies on the profile
    // itself, and `CompileError::MissingTagService` is raised only on the
    // `(Selector::Tag, EngineType::Trino)` branch
    // (`egret-perm-compile/src/rules.rs`). Applying this guard to StarRocks
    // would refuse every legitimate StarRocks bind — including the one the
    // live masking gate depends on — while protecting nothing.
    if svc.r#type == "trino"
        && st
            .authz
            .usable_tag_service(sid)
            .await
            .map_err(|e| CpError::BadRequest(format!("resolve tag service: {e}")))?
            .is_none()
    {
        return Err(CpError::BadRequest(format!(
            "policy profile {sid} ({}) has no usable tag service (its `tagService` is \
             unset, or names a profile that does not exist), so every tag-based column \
             mask and row filter would silently compile to nothing; give the profile a \
             tag service, or bind one created by engine provisioning",
            svc.name
        )));
    }
    Ok(())
}

/// Apply an update body's `authz_service_id` to `engine`, binding it to that
/// policy profile and compiling the profile in the SAME transaction.
///
/// **Why honoured and not refused.** `CreateEngineBody` is shared with create,
/// and `update_engine` refuses its `catalogs` field outright because accepting
/// and dropping it would tell the caller their catalogs were attached when
/// they were not — and because `POST /engines/{id}/catalogs/{cid}` is a real
/// route to send them to instead. `authz_service_id` had the same silence with
/// no such route behind it: the field was declared, accepted and dropped, so
/// the only way to bind an engine to an existing profile after create was a
/// direct SQL `UPDATE`. This is that route.
///
/// **`None` means "unchanged", never "unbind".** The wire shape cannot tell an
/// absent field from an explicit `null` (`#[serde(default)]` on an
/// `Option<i64>`), and the update wizard sends the whole body — so treating
/// `None` as a detach would silently unbind every engine on every unrelated
/// spec edit. That is a worse silence than the one being fixed. Changing the
/// binding from profile A to profile B is a plain re-point: A's row is
/// untouched, because unbinding must never delete a profile.
///
/// **An unknown id is a 400 naming it.** Not a warning-and-carry-on the way
/// create's `plan_trino_profile` treats a bad id (there, a fresh profile is
/// created instead, so the engine still ends up governed). Here there is no
/// fallback, and a silent no-op is precisely the defect.
///
/// **Bind and compile commit together.** A profile an engine newly binds is
/// stale: the compiler's affected-service sets are all derived from the
/// MUTATED model row, and a binding mutates no grant and no rule. SP4a fixed
/// this at engine create (`680adea9`); this is the same fix on update, through
/// the same seam — `ProfileCompilerPort::recompile_profile_in_tx`, which locks
/// through `lock_services_in_tx` (expanded with each profile's tag service,
/// ascending). Unlike create — where a cluster is already provisioned by the
/// time the recompile runs, so a failure only logs — nothing has been applied
/// here yet, so a compile failure rolls the binding back and refuses.
async fn apply_authz_service_update(
    st: &CpState,
    engine: &mut Engine,
    requested: Option<i64>,
) -> Result<(), CpError> {
    let Some(sid) = requested else {
        return Ok(());
    };
    if engine.authz_service_id == Some(sid) {
        return Ok(()); // already bound there; no write, nothing to recompile
    }
    check_profile_bindable(st, sid, &engine.r#type, &engine.name).await?;
    // The binding write and the compile of the profile it names share ONE
    // transaction, the same shape `catalogs::set_catalog_workspaces` keeps —
    // and, like it, `sqlx::Error` from `.begin()`/`.commit()` is mapped
    // through `db_error` explicitly at the call site rather than through a
    // blanket `From`.
    let mut tx = st
        .engine_mgmt
        .pool()
        .begin()
        .await
        .map_err(crate::http_error::db_error)?;
    // **The pairs this bind creates are refused** (SP4d-4 Task 3 re-review M-2).
    // Binding changes which catalogs — and so which tag assignments — the
    // profile's unit resolves, and every overlap check reads those as stored, so
    // the snapshot of the unit's pairs is taken BEFORE the bind and compared
    // after the recompile. A pair already stored is reported, not refused.
    let overlaps_before =
        crate::profile_compiler::snapshot_overlaps(st, &mut tx, Some(sid)).await?;
    if !st
        .engine_mgmt
        .set_engine_authz_service_in_tx(&mut tx, engine.id, Some(sid), &engine.status)
        .await?
    {
        // The row moved since it was read — typically a concurrent delete
        // flipping it to `deleting`. Same compare-and-set, and same 409, the
        // spec write uses; the transaction is dropped, so nothing is bound and
        // nothing is compiled.
        return Err(CpError::Conflict(
            "engine changed while the policy-profile binding was being applied; retry".into(),
        ));
    }
    if let Err(e) = crate::profile_compiler::recompile_refusing_created_overlaps(
        st,
        &mut tx,
        sid,
        overlaps_before.as_ref(),
    )
    .await
    {
        tracing::error!(
            engine = %engine.name,
            service_id = sid,
            error = %e,
            "compiling the policy profile this engine was being bound to failed; \
             the binding was rolled back rather than committed uncompiled"
        );
        // An overlap this bind created is the caller's 409; anything else is ours.
        return Err(match e.rule_overlap_body() {
            Some(body) => CpError::RuleOverlap(body),
            None => CpError::Internal,
        });
    }
    tx.commit().await.map_err(crate::http_error::db_error)?;
    tracing::info!(
        engine = %engine.name,
        previous = ?engine.authz_service_id,
        service_id = sid,
        "engine re-pointed at a policy profile by update; the previous profile row is untouched"
    );
    engine.authz_service_id = Some(sid);
    Ok(())
}

#[utoipa::path(
    put,
    path = "/api/control-plane/engines/{id}",
    tag = "control-plane-engines",
    params(("id" = Uuid, Path, description = "Engine id")),
    request_body = CreateEngineBody,
    responses(
        (status = 200, description = "Engine spec updated and pods rolled", body = EngineDto),
        (status = 400, description = "Invalid config, Kubernetes not configured, `catalogs` sent (create-only — attach through POST /engines/{id}/catalogs/{cid}), `starrocks.feMetaStorageClass`/`feMetaStorageSize` sent (create-only — they render into the FE StatefulSet's immutable `volumeClaimTemplates`; the message says what to do instead), `authz_service_id` naming no policy profile or one with no usable tag service, or re-materializing workspace connections failed", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:update permission", body = ErrorBody),
        (status = 404, description = "No such engine", body = ErrorBody),
        (status = 409, description = "The engine changed concurrently (e.g. a delete in progress), or `authz_service_id` names a profile of the wrong type or one another engine already binds; retry; The engine is external (`code` = `engine_is_external`): egret does not update, restart, scale or delete the cluster of it", body = ErrorBody),
        (status = 502, description = "Kubernetes rejected the update; the previous spec was restored", body = ErrorBody),
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
    // `CreateEngineBody` is shared with create, but update has never attached
    // anything and does not start here. Accepting the field and dropping it
    // would tell the caller their catalogs were attached (review F8); refusing
    // sends them to the route that actually does it.
    if !b.catalogs.is_empty() {
        return Err(CpError::BadRequest(
            "`catalogs` is not accepted on update: attach with \
             POST /api/control-plane/engines/{id}/catalogs/{cid}"
                .into(),
        ));
    }
    let mut engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    // An external engine has no spec of egret's to update, and `prov.update`
    // would apply one to a cluster egret does not own. Its address and plugin
    // version are changed through the dedicated PATCH route.
    crate::external::refuse_if_external(
        &engine,
        "apply a provisioning spec to it; change its address, declared plugin version or \
         policy profile with PATCH /api/control-plane/engines/{id}/external (or delete it first \
         to register it again)",
    )?;
    // I4: EVERY cheap refusal runs BEFORE the profile binding is committed.
    // The bind is a real write (and compiles policy), so a PUT that ends in a
    // 400 or a 409 must not have performed it. These three guards used to sit
    // downstream of the bind, inside the type-specific paths.
    //
    //   1. the engine kind — an unsupported kind is a 400 either way;
    //   2. kubernetes configured — both paths refuse without a provisioner, so
    //      hoisting the check changes no outcome, only its order (each path
    //      keeps its own check, now unreachable but still correct);
    //   3. the StarRocks body validators, which are pure functions of the body
    //      (delegation mode, protected `feConfig` keys, autoscaling bounds) and
    //      so are safe to evaluate twice.
    //
    // The bind's own write is additionally compare-and-set on the status read
    // above, so the concurrent-delete 409 cannot land after it either.
    if engine.r#type != "starrocks" && engine.r#type != "trino" {
        return Err(CpError::BadRequest(
            "update not supported for this engine kind".into(),
        ));
    }
    if st.engine_provisioner.is_none() {
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    }
    if engine.r#type == "trino" {
        refuse_dropped_trino_image(&b)?;
    }
    if engine.r#type == "starrocks" {
        let sr_body = b.starrocks.clone().unwrap_or_default();
        validate_sr_delegation_mode(&sr_body)?;
        validate_sr_fe_config(&sr_body)?;
        validate_sr_autoscaling(&sr_body)?;
        validate_sr_fe_meta_storage(&sr_body)?;
        // The FE metadata volume is CREATE-ONLY. Hoisted here with the others so
        // the refusal lands before the `authz_service_id` bind writes anything.
        validate_sr_fe_meta_immutable(&sr_body, &engine.spec)?;
    }
    // `authz_service_id` is HONOURED here rather than refused the way
    // `catalogs` is above, because there is no other route to send the caller
    // to: until this landed, binding an engine to an existing policy profile
    // after create was only reachable by a direct SQL `UPDATE` (which is what
    // SP4a's live gate had to do). Applied before the type dispatch so the
    // provision request the dispatch builds — whose `RangerBinding` is derived
    // from `engine.authz_service_id` — sees the new binding.
    let previous_authz_service_id = engine.authz_service_id;
    apply_authz_service_update(&st, &mut engine, b.authz_service_id).await?;
    let rebound = engine.authz_service_id != previous_authz_service_id;
    // StarRocks has its own spec builder + provision-req assembler (FE/CN roles,
    // builtin storage volume) — dispatch to it rather than running the Trino
    // spec/assembly below.
    let engine_id = engine.id;
    let out = if engine.r#type == "starrocks" {
        update_starrocks_engine(st.clone(), engine, b).await
    } else {
        update_trino_engine(st.clone(), engine, b).await
    };
    // A refusal from further down — one decided from the BUILT spec, which the
    // hoisted guards above cannot pre-empt — must not leave the engine
    // re-pointed. See `restore_engine_authz_service`.
    if out.is_err() && rebound {
        restore_engine_authz_service(&st, engine_id, previous_authz_service_id).await;
    }
    // The update stood: the profile the engine left must stop serving what the
    // engine's grants compiled there (spec E10).
    if out.is_ok() && rebound {
        recompile_former_profile(&st, previous_authz_service_id, false).await;
    }
    out
}

/// Trino update path, split out from [`update_engine`] (which only does the
/// auth check, the fetch, and the type dispatch) so it takes the `Engine` it
/// was dispatched with directly — the same shape [`update_starrocks_engine`]
/// already has — and can be driven in a test with a snapshot read before a
/// simulated concurrent change, without needing true task interleaving.
async fn update_trino_engine(
    st: CpState,
    engine: Engine,
    b: CreateEngineBody,
) -> Result<Json<EngineDto>, CpError> {
    let id = engine.id;
    // An update is only meaningful if it can be applied: refuse BEFORE
    // persisting, so the stored spec never runs ahead of the cluster.
    let Some(prov) = st.engine_provisioner.clone() else {
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };

    // Rebuild the spec from the body + config. The primary Catalog Client is NOT
    // re-minted (its credential lives on the primary catalog row), so nothing is
    // revoked here. Trino has no hot-reload; the provisioner rolls the pods.
    // Body value, else the image this engine already runs: an update that omits
    // the image (a resize) must not move the engine to the current default.
    let image_version = match b.image_version.clone().filter(|s| !s.trim().is_empty()) {
        Some(v) => v,
        None => stored_image(&engine, &["imageVersion"])?,
    };
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
    preserve_trino_delegation_mode(&mut spec, &engine.spec);
    validate_spec_resource_groups(&spec)?;
    // Validate an optional ingress-TLS secret reference (400 on a missing /
    // non-TLS / incomplete secret) before persisting, mirroring create.
    if let Some(ing) = parse_ingress(&spec["trinoConfig"]["ingress"]) {
        if let Some(ref_id) = ing.tls_cert_secret_ref {
            resolve_tls_cert_secret(&st, ref_id).await?;
        }
    }
    // Validate an optional image-pull (dockerconfigjson) secret reference the
    // same way (400 on a missing / non-dockerconfigjson / incomplete secret)
    // before persisting, mirroring create.
    if let Some(ref_id) = spec["trinoConfig"]["imagePullSecretRef"]
        .as_str()
        .and_then(|s| s.parse::<Uuid>().ok())
    {
        resolve_dockerconfigjson_secret(&st, ref_id).await?;
    }

    // Snapshot what is stored so a failed build or apply can put it back — the
    // same contract `update_catalog` keeps for a catalog row.
    let snapshot_spec = engine.spec.clone();
    let expected_status = engine.status.clone();
    // Compare-and-set on the status just read. A miss means the row changed
    // since (typically a concurrent delete) — stop here, BEFORE the
    // provisioner is touched at all, so a lost race can never re-create the
    // CR of an engine whose row is already gone (the orphaned-cluster class
    // this programme closes).
    if !st
        .engine_mgmt
        .update_engine(id, &spec, &expected_status)
        .await?
    {
        return Err(CpError::Conflict(
            "engine changed while the update was being applied; retry".into(),
        ));
    }
    let engine = match st.engine_mgmt.get_engine(id).await {
        Ok(Some(e)) => e,
        Ok(None) => return Err(CpError::NotFound),
        Err(e) => {
            restore_engine_spec(&st, id, &snapshot_spec, &expected_status).await;
            return Err(e.into());
        }
    };
    let built = match build_provision_req(&st, &engine).await {
        Ok(built) => built,
        Err(e) => {
            restore_engine_spec(&st, id, &snapshot_spec, &expected_status).await;
            return Err(e);
        }
    };
    if let Err(e) = apply_built(&st, prov.as_ref(), &engine, &built, ApplyOp::Update).await {
        restore_engine_spec(&st, id, &snapshot_spec, &expected_status).await;
        return Err(match e {
            ApplyBuiltError::Conflict => apply_conflict(),
            ApplyBuiltError::Failed(msg) => CpError::ClusterApplyFailed(msg),
        });
    }

    // Re-materialize every assigned workspace's connection so a delegation-mode
    // or TLS change reaches the query path — as `update_starrocks_engine` does,
    // and unconditionally for the same reason: a retried PUT must heal a
    // previous partial re-materialization, which a diff against the
    // already-persisted spec cannot see. Idempotent and cheap when unchanged.
    rematerialize_or_400(&st, id).await?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
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
    // ...and the FE metadata volume, which is CREATE-ONLY: both knobs render
    // inside the FE StatefulSet's immutable `volumeClaimTemplates`. Duplicated
    // from the hoisted block in `update_engine` the same way the three above
    // are — unreachable through the handler, still correct on its own.
    validate_sr_fe_meta_storage(&sr_body)?;
    validate_sr_fe_meta_immutable(&sr_body, &engine.spec)?;
    // Validate an optional FE-ingress-TLS secret reference up front (400 on a
    // missing / non-TLS / incomplete secret), mirroring the shared create-time
    // guard — the StarRocks update path dispatches before that Trino-only check.
    if let Some(ing) = b
        .trino_config
        .as_ref()
        .and_then(|tc| parse_ingress(&tc["ingress"]))
    {
        if let Some(ref_id) = ing.tls_cert_secret_ref {
            resolve_tls_cert_secret(&st, ref_id).await?;
        }
    }

    // Capture the OLD delegation block ({mode, tlsEnabled} or null) from the
    // pre-update engine before we overwrite the spec, so we can detect a
    // delegation/TLS change below.
    let old_delegation = sr_delegation_block(&engine.spec);

    // Merge the body over the CURRENT spec so a PUT that omits image/replicas/
    // resources (or delegation) preserves what's already stored.
    // An image the body omits is kept from the stored spec; with neither, refuse
    // rather than default to the current release.
    for (field, body_value) in [
        ("feImage", &sr_body.fe_image),
        ("cnImage", &sr_body.cn_image),
    ] {
        if body_value.as_deref().is_none_or(|s| s.trim().is_empty()) {
            stored_image(&engine, &["starrocksConfig", field])?;
        }
    }
    let mut spec = build_starrocks_spec(&sr_body, Some(&engine.spec));
    // Preserve the top-level FE ingress across the merge (mirrors how
    // `build_starrocks_spec` falls back to `existing` for omitted fields): a PUT
    // body carrying an ingress wins; otherwise keep whatever the stored spec had.
    if let Some(ing) = b.trino_config.as_ref().and_then(|tc| tc.get("ingress")) {
        spec["ingress"] = ing.clone();
    } else if let Some(existing) = engine.spec.get("ingress") {
        spec["ingress"] = existing.clone();
    }
    // A delegation change flips the connector's auth mode (impersonate ⇄ jwt) or
    // its in-band TLS, so it must re-bootstrap the FE. Compare the full stored
    // `delegation` block before vs. after (see `sr_delegation_block`); a spurious
    // match-miss only costs an idempotent no-op re-bootstrap. This gates ONLY the
    // FE re-bootstrap — re-materialization runs unconditionally below.
    let deleg_changed = old_delegation != sr_delegation_block(&spec);
    // Refuse BEFORE persisting when nothing can apply the update.
    let Some(prov) = st.engine_provisioner.clone() else {
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };
    // Re-resolve the warehouse the engine leads with — its primary attachment's
    // (`primary_warehouse`, the same resolution `build_provision_req_with`'s
    // StarRocks arm uses) — so the FE/CN builtin storage volume is fed the SAME
    // S3 coords it was provisioned with. It does not depend on the new spec, so
    // resolve it before persisting too: an engine with no primary is refused
    // here, with its spec untouched and nothing to restore.
    let wh = primary_warehouse(&st, &engine).await?;

    // Snapshot for restore on a failed build or apply (see `update_engine`).
    let snapshot_spec = engine.spec.clone();
    let expected_status = engine.status.clone();
    let engine_id = engine.id;
    // Compare-and-set on the status just read. A miss means the row changed
    // since (typically a concurrent delete) — stop here, BEFORE the
    // provisioner is touched at all (see `update_engine`'s matching guard).
    if !st
        .engine_mgmt
        .update_engine(engine_id, &spec, &expected_status)
        .await?
    {
        return Err(CpError::Conflict(
            "engine changed while the update was being applied; retry".into(),
        ));
    }
    let engine = match st.engine_mgmt.get_engine(engine_id).await {
        Ok(Some(e)) => e,
        Ok(None) => return Err(CpError::NotFound),
        Err(e) => {
            restore_engine_spec(&st, engine_id, &snapshot_spec, &expected_status).await;
            return Err(e.into());
        }
    };
    let (req, bootstrap) = match build_starrocks_provision_req(&st, &engine, &wh).await {
        Ok(v) => v,
        Err(e) => {
            restore_engine_spec(&st, engine_id, &snapshot_spec, &expected_status).await;
            return Err(e);
        }
    };
    if let Err(msg) = prov.update(&req).await {
        restore_engine_spec(&st, engine_id, &snapshot_spec, &expected_status).await;
        return Err(CpError::ClusterApplyFailed(msg));
    }
    retire_ranger_previous_after_apply(&st, engine_id, &req).await;

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
        // It reads its own credentials now (SP3b Task 5: the convergence
        // resolves each attachment's secret itself), so the primary-credential
        // lookup this used to do — and the arm that SKIPPED the whole
        // re-bootstrap when it found none — are gone: there is nothing left for
        // that arm to protect, and a missing primary credential must not cost
        // the engine its JWT integration.
        if let Some(boot) = bootstrap {
            spawn_starrocks_catalog_wire(
                st.clone(),
                engine.id,
                engine.name.clone(),
                engine.namespace.clone(),
                Some(boot),
            );
        }
    }
    // (b) Re-materialize the per-workspace connections unconditionally.
    rematerialize_or_400(&st, engine.id).await?;
    Ok(Json(engine.into()))
}

/// The full `delegation` sub-object of a StarRocks engine spec (today
/// `{mode, tlsEnabled}`), or `Null` when absent. Used to detect a delegation/TLS
/// change across an update: an inequality between the pre- and post-update blocks
/// means the connector's auth mode or in-band TLS flipped. Comparing the WHOLE
/// block (not just `mode`/`tlsEnabled`) is the safe side — at worst a future
/// extra key triggers a spurious FE re-bootstrap, which is an idempotent no-op.
/// Whether this StarRocks engine's FE actually serves TLS on its MySQL port:
/// delegation is configured AND its `tlsEnabled` has not been turned off —
/// which DEFAULTS TO ON, because that is the shape `build_starrocks_provision_req`
/// provisions (it mints the cert and hands the operator a TLS listener unless
/// the stored block opted out).
///
/// One function, called by both the provisioning side and
/// [`crate::starrocks_ddl::fe_target`] (SP3b Task 5), because the two had
/// silently disagreed: `fe_target` read `tlsEnabled` with `unwrap_or(false)`
/// and ignored `mode` entirely, so the common engine — delegation on, no
/// explicit `tlsEnabled` — was provisioned WITH TLS and then dialled WITHOUT
/// the pinned certificate. Task 5 moves the delegation DDL onto `fe_target`'s
/// one target (D6); leaving the two predicates apart would have downgraded
/// every such engine's statements to cleartext on the way.
pub(crate) fn sr_tls_enabled(spec: &serde_json::Value) -> bool {
    let delegation = spec
        .get("starrocksConfig")
        .and_then(|c| c.get("delegation"));
    let mode_configured = delegation
        .and_then(|d| d.get("mode"))
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty());
    mode_configured
        && delegation
            .and_then(|d| d.get("tlsEnabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true)
}

pub(crate) fn sr_delegation_block(spec: &serde_json::Value) -> serde_json::Value {
    spec.get("starrocksConfig")
        .and_then(|c| c.get("delegation"))
        .cloned()
        .unwrap_or(serde_json::Value::Null)
}

#[utoipa::path(
    delete,
    path = "/api/control-plane/engines/{id}",
    tag = "control-plane-engines",
    params(("id" = Uuid, Path, description = "Engine id")),
    responses(
        (status = 204, description = "For an external engine: egret's rows only (the engine, its declared attachments and its profile binding) are removed and nothing is sent to any cluster or provisioner, and Kubernetes need not be configured. For a provisioned engine: cluster deleted, engine row removed (its catalogs, workspace connections, query history and notebook access cascade), engine-generated secrets and minted Catalog Clients cleaned up; the engine's own Ranger profile is unbound, not deleted"),
        (status = 400, description = "Kubernetes not configured", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:delete permission", body = ErrorBody),
        (status = 404, description = "No such engine", body = ErrorBody),
        (status = 409, description = "Nothing deleted: Data API endpoints (including trashed), schedules or datasets still use the engine's workspace connections (`code` = `engine_has_dependents`, with `dependents`), or a database constraint refused the delete (`code` = `constraint_violation`, with `constraint`)", body = EngineConflictBody),
        (status = 500, description = "Internal error", body = ErrorBody),
        (status = 502, description = "The Kubernetes delete failed (`code` = `engine_cluster_delete_failed`); the engine stays `deleting` and the request can be retried", body = EngineConflictBody),
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

    // 1. Preflight. Nothing is touched while a user-authored asset still sits on
    //    one of this engine's workspace connections: the row delete would fail on
    //    it (endpoints RESTRICT, alerts CHECK) or silently destroy it (datasets).
    let dependents = st.engine_mgmt.engine_dependents(id, None).await?;
    if !dependents.is_empty() {
        return Err(CpError::HasDependents(dependents));
    }

    // An external engine is egret's rows and nothing else: no cluster to
    // delete, so no Kubernetes requirement, no provisioner call and no
    // `deleting` state to retry. The same preflight above has already refused
    // while a dependent exists.
    if crate::external::is_external(&engine) {
        return delete_external_engine(&st, &engine).await;
    }

    // 2. Same rule as create: without Kubernetes the cluster cannot be removed,
    //    and deleting only the row would orphan it.
    let Some(prov) = st.engine_provisioner.as_ref() else {
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };

    // Attachments (minted clients, credential secrets), read before they cascade
    // away with the row.
    let attachments = st
        .engine_mgmt
        .registry()
        .list_engine_attachments(engine.id)
        .await?;

    // 3. Visible, retryable state for everything below that can fail.
    st.engine_mgmt
        .set_engine_status(id, crate::engine_status::DELETING)
        .await?;

    // 4. The cluster. A 404 is already success inside the provisioners.
    if let Err(msg) = prov
        .delete(&engine.r#type, &engine.namespace, &engine.name)
        .await
    {
        tracing::error!(engine = %engine.name, error = %msg, "engine cluster delete failed; row left deleting");
        return Err(CpError::ClusterDeleteFailed(msg));
    }

    // 5. The row, re-checking dependents under locks (a dependent created since
    //    step 1 answers 409; the row stays `deleting` and a retry finishes).
    match st.engine_mgmt.delete_engine_unless_blocked(id).await? {
        DeleteEngineOutcome::Deleted | DeleteEngineOutcome::NotFound => {}
        DeleteEngineOutcome::Blocked(dependents) => {
            tracing::warn!(
                engine = %engine.name,
                "dependents appeared after the cluster delete; row left deleting"
            );
            return Err(CpError::HasDependents(dependents));
        }
    }

    // 6. Cross-seam cleanup after commit: logged, never returned.
    cleanup_deleted_engine(&st, &engine, &attachments).await;
    // 7. A profile another engine still binds loses the engine grants that
    //    cascaded with the row (spec E10).
    recompile_former_profile(&st, engine.authz_service_id, true).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Delete the rows egret holds for an external engine: the engine, its
/// declared attachments and its profile binding (the profile itself stays, as
/// for a provisioned engine). **Never calls the provisioner.**
///
/// An external engine owns no secret egret connects WITH
/// (`engines_external_shape` keeps every such column NULL) and a declared
/// attachment minted no client (`attachments_declared_holds_nothing`), so there
/// is nothing for [`cleanup_deleted_engine`] to remove, and a secret the
/// cluster might still hold must never be deleted. That is asserted here rather
/// than assumed: a row that somehow carries one is refused untouched.
///
/// The one exception is the engine's Ranger plugin credential (spec
/// 2026-10-20-ranger-surface-auth A11), which only authenticates THIS engine
/// to egret: once the row is gone it verifies for nobody, so it is deleted
/// with the row ([`delete_ranger_credentials`]). It is deliberately not in
/// the guard above.
async fn delete_external_engine(st: &CpState, engine: &Engine) -> Result<StatusCode, CpError> {
    let attachments = st
        .engine_mgmt
        .registry()
        .list_engine_attachments(engine.id)
        .await?;
    let holds_a_credential = attachments.iter().any(|a| {
        a.attachment.minted_client
            || a.attachment.catalog_client_id.is_some()
            || a.attachment.credential_secret_id.is_some()
    });
    if !engine_owned_secret_ids(engine).is_empty()
        || engine.tls_cert_secret_ref.is_some()
        || holds_a_credential
    {
        tracing::error!(engine = %engine.name,
            "an external engine row carries a secret or a minted client; refusing to delete it \
             rather than leave or remove a credential the cluster may hold");
        return Err(CpError::Internal);
    }
    match st
        .engine_mgmt
        .delete_engine_unless_blocked(engine.id)
        .await?
    {
        DeleteEngineOutcome::Deleted | DeleteEngineOutcome::NotFound => {}
        DeleteEngineOutcome::Blocked(dependents) => return Err(CpError::HasDependents(dependents)),
    }
    delete_ranger_credentials(st, engine).await;
    // The profile this engine was bound to loses its engine with the row. Its
    // catalog grants stay the profile's own; its ENGINE grants cascaded with
    // the row, so a profile another engine still binds is recompiled (spec E10).
    recompile_former_profile(st, engine.authz_service_id, true).await;
    tracing::info!(engine = %engine.name,
        "external engine deleted; egret's rows only, nothing was sent to the cluster");
    Ok(StatusCode::NO_CONTENT)
}

/// After an engine's row is deleted: its Ranger plugin credentials, by the
/// exact ids the row named (current and previous) and then by the engine's
/// own name prefix `engine-ranger-{id}-` (generations a lost compare-and-set
/// or a failed delete left behind). The prefix ends in the full engine id and
/// a hyphen, so it cannot match another engine's credential. Failures are
/// logged: the row is already gone, so none of these verifies any more.
async fn delete_ranger_credentials(st: &CpState, engine: &Engine) {
    delete_secret_ids(st, engine, engine_ranger_secret_ids(engine)).await;
    sweep_secret_prefixes(
        st,
        engine,
        &[crate::ranger_credential::secret_name_prefix(engine.id)],
    )
    .await;
}

/// Delete each of `ids` from the global secret scope; a missing one is fine.
async fn delete_secret_ids(st: &CpState, engine: &Engine, ids: impl IntoIterator<Item = Uuid>) {
    for sid in ids {
        match st.secrets.delete(None, sid).await {
            Ok(()) | Err(egret_secrets::SecretsError::NotFound) => {}
            Err(e) => {
                tracing::warn!(engine = %engine.name, secret = %sid, error = %e, "delete engine secret failed")
            }
        }
    }
}

/// Delete every global secret whose name starts with one of `prefixes`
/// (plain `starts_with`, never SQL `LIKE`). Engine delete only: see
/// [`cleanup_deleted_engine`] for why a sweep is safe there and nowhere else.
async fn sweep_secret_prefixes(st: &CpState, engine: &Engine, prefixes: &[String]) {
    match st.secrets.list(None).await {
        Ok(all) => {
            for meta in all {
                if !prefixes.iter().any(|p| meta.name.starts_with(p.as_str())) {
                    continue;
                }
                match st.secrets.delete(None, meta.id).await {
                    Ok(()) | Err(egret_secrets::SecretsError::NotFound) => {}
                    Err(e) => {
                        tracing::warn!(engine = %engine.name, secret = %meta.id, error = %e,
                            "delete orphaned engine secret generation failed")
                    }
                }
            }
        }
        Err(e) => {
            tracing::warn!(engine = %engine.name, error = %e, "list secrets for orphan generation sweep failed")
        }
    }
}

/// What an engine leaves outside its own tables. Each seam owns its own pool, so
/// none of this can share the row delete's transaction; a failure here is logged
/// and leaves an orphan, never a half-deleted engine.
async fn cleanup_deleted_engine(st: &CpState, engine: &Engine, attachments: &[AttachedCatalog]) {
    // Only clients we minted, and the named secrets holding their credentials
    // (egret's copies; the cluster that used them is gone). `minted_client =
    // false` belongs to someone else — its client AND its credential secret,
    // which may be shared with another engine's attachment, stay.
    for a in attachments {
        crate::catalog_apply::retire_minted_credential(st, engine, &a.attachment).await;
    }
    // Secrets egret generated for this engine — the same ownership list create
    // compensation uses (Task 12's `engine_owned_secret_ids`). This already
    // covers `internal_secret_id`/`monitor_secret_id` (Task 6). Never
    // `tls_cert_secret_ref`: admin-provided and possibly shared with other
    // engines.
    // The Ranger plugin credential (current and previous) goes with them.
    delete_secret_ids(
        st,
        engine,
        engine_owned_secret_ids(engine)
            .into_iter()
            .chain(engine_ranger_secret_ids(engine)),
    )
    .await;
    // Orphan generations a failed rotation leaves behind. `commit_internal_secrets`
    // mints a fresh `engine-internal-{id}-{generation}` / `engine-monitor-{id}-
    // {generation}` pair on every rotation; when the apply that carried it never
    // reaches the cluster (`Supersede::Keep`), or the compare-and-set that would
    // record it on the row loses a race, that pair's rows are never referenced by
    // anything and nothing sweeps them — see that function's doc comment for why
    // no earlier point is safe to do so (a concurrent apply on this engine could
    // still be relying on them). Sweeping by a prefix of the engine's id is safe
    // HERE and ONLY here: the engine row is already gone and its delete already
    // went through `deleting`, so no apply for this engine id can still be in
    // flight to race this sweep.
    //
    // The same holds for `engine-ranger-{id}-*`: a credential a lost
    // compare-and-set or a failed delete left behind (see
    // `crate::ranger_credential`); after the row is gone no plugin can
    // authenticate with any of them anyway.
    sweep_secret_prefixes(
        st,
        engine,
        &[
            format!("engine-internal-{}-", engine.id),
            format!("engine-monitor-{}-", engine.id),
            crate::ranger_credential::secret_name_prefix(engine.id),
        ],
    )
    .await;
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
        (status = 409, description = "The engine is external (`code` = `engine_is_external`): egret does not restart it", body = ErrorBody),
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
    crate::external::refuse_if_external(&engine, "restart it")?;
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

/// `POST /engines/{id}/rotate-secrets` — mint a new Trino internal-communication
/// secret and `egret-monitor` password and re-apply the engine (pods roll). The
/// new pair is recorded on the row even if the apply fails — the cluster may
/// already have received it — so a failed rotation's pair is kept resolvable,
/// never deleted, alongside whatever it superseded. A later successful
/// rotation deletes only the ONE pair its own apply superseded; anything
/// further back (left by an earlier failed rotation, or by a losing
/// concurrent apply) stays orphaned but resolvable until the engine itself is
/// deleted — a wider sweep here could destroy a pair a DIFFERENT, still
/// in-flight apply on this engine just minted, which is the one thing this
/// route must never do.
#[utoipa::path(
    post,
    path = "/api/control-plane/engines/{id}/rotate-secrets",
    tag = "control-plane-engines",
    params(("id" = Uuid, Path, description = "Engine id")),
    responses(
        (status = 204, description = "New internal secrets applied; the engine's pods restart"),
        (status = 400, description = "Not a Trino engine, or Kubernetes not configured", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:update permission", body = ErrorBody),
        (status = 404, description = "No such engine", body = ErrorBody),
        (status = 409, description = "The engine changed concurrently (e.g. a delete in progress); retry; The engine is external (`code` = `engine_is_external`): egret does not rotate the secrets of it", body = ErrorBody),
        (status = 502, description = "Kubernetes rejected the apply; the engine row now names the new secret pair (the cluster may already hold it), and the previous pair is kept — not deleted — until a later successful apply supersedes it", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn rotate_engine_secrets(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:update").await?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    crate::external::refuse_if_external(&engine, "rotate its secrets")?;
    // A concurrent DELETE already marked the row and may have torn the cluster
    // down; 409 the common case here, before doing any work, rather than only
    // catching it in `apply_built`'s re-read right before the provisioner call.
    if engine.status == crate::engine_status::DELETING {
        return Err(apply_conflict());
    }
    // Only Trino has these two secrets; StarRocks' analogues (root and
    // delegator passwords) are separate and not rotated here.
    if engine.r#type != "trino" {
        return Err(CpError::BadRequest(format!(
            "rotating internal secrets is not supported for {} engines",
            engine.r#type
        )));
    }
    let Some(prov) = st.engine_provisioner.as_ref() else {
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };
    let built = build_provision_req_with(
        &st,
        &engine,
        BuildOpts {
            rotate_internal_secrets: true,
            ..BuildOpts::default()
        },
    )
    .await?;
    apply_built(&st, prov.as_ref(), &engine, &built, ApplyOp::Update)
        .await
        .map_err(|e| match e {
            ApplyBuiltError::Conflict => apply_conflict(),
            ApplyBuiltError::Failed(msg) => CpError::ClusterApplyFailed(msg),
        })?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /engines/{id}/rotate-ranger-credential` — mint a new Ranger plugin
/// credential. Spec 2026-10-20-ranger-surface-auth A10/A11: the new credential
/// is committed and the old one demoted to previous; both authenticate until
/// the previous one is retired. A rotation is refused (409) while a previous
/// one is still pending, so a rotation never deletes a credential.
///
/// **Provisioned engine** (Trino or StarRocks): the new credential is applied
/// (the pods roll) and the old one is deleted by its exact id as soon as
/// Kubernetes ACCEPTS the apply — which can be before the pods have finished
/// rolling, so a pod still running with the old password can be refused by
/// `/ranger` until it is replaced. A failed apply deletes nothing, and any
/// later accepted apply retires the previous one. Answers 204.
///
/// **External engine**: nothing is applied (egret never touches its cluster).
/// The answer is 200 with the new credential — **shown once** — for the
/// operator to install. No apply can prove they did, so the previous one stays
/// valid until the operator confirms with
/// `POST /engines/{id}/retire-previous-ranger-credential`.
#[utoipa::path(
    post,
    path = "/api/control-plane/engines/{id}/rotate-ranger-credential",
    tag = "control-plane-engines",
    params(("id" = Uuid, Path, description = "Engine id")),
    responses(
        (status = 200, description = "External engine: the new Ranger plugin credential, shown only here. The previous credential still authenticates until `POST /api/control-plane/engines/{id}/retire-previous-ranger-credential` retires it; until then a further rotation is refused", body = crate::external::RangerCredentialDto),
        (status = 204, description = "Provisioned engine: Kubernetes accepted the apply of a new Ranger plugin credential; the engine's pods restart. The previous credential is retired as soon as the apply is accepted, which can be before the pods have finished rolling: a pod still running with the old password can be refused by the Ranger surface until it is replaced"),
        (status = 400, description = "Provisioned engine bound to no policy profile (it has no Ranger credential), or Kubernetes is not configured", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:update permission", body = ErrorBody),
        (status = 404, description = "No such engine", body = ErrorBody),
        (status = 409, description = "Nothing was rotated, for one of two reasons: a previous rotation is still pending (provisioned: its credential has not been retired by an accepted apply yet, and any accepted apply retires it; external: retire it with `POST /api/control-plane/engines/{id}/retire-previous-ranger-credential` once the new one is installed); or the engine changed concurrently (e.g. a delete in progress)", body = ErrorBody),
        (status = 502, description = "Provisioned engine: Kubernetes rejected the apply; the engine row now names the new credential and keeps the previous one — both still authenticate — until a later accepted apply retires it", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn rotate_ranger_credential(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Response, CpError> {
    use axum::response::IntoResponse as _;
    egret_core::authz::require(&*st.pdp, &p, "engine:update").await?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    if crate::external::is_external(&engine) {
        // The row alone: an external engine's cluster is never touched. An
        // engine registered before credentials existed has no current one, so
        // this mints its first.
        let credential = crate::ranger_credential::rotate(&st, &engine).await?;
        tracing::info!(engine = %engine.name,
            "external engine's ranger credential rotated; the previous one stays valid until \
             the operator retires it");
        return Ok(Json(crate::external::RangerCredentialDto {
            username: credential.username.clone(),
            password: credential.password.clone(),
        })
        .into_response());
    }
    if engine.status == crate::engine_status::DELETING {
        return Err(apply_conflict());
    }
    // An unbound engine renders no Ranger binding, so there is no credential to
    // rotate. A Trino engine late-binds a profile on any apply once Ranger is
    // on (`build_provision_req_with`), so it counts as bound then.
    let bound =
        engine.authz_service_id.is_some() || (st.ranger_enabled && engine.r#type == "trino");
    if !bound {
        return Err(CpError::BadRequest(format!(
            "engine {} is bound to no policy profile: it has no ranger credential",
            engine.name
        )));
    }
    let Some(prov) = st.engine_provisioner.as_ref() else {
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };
    let built = build_provision_req_with(
        &st,
        &engine,
        BuildOpts {
            rotate_ranger_credential: true,
            ..BuildOpts::default()
        },
    )
    .await?;
    apply_built(&st, prov.as_ref(), &engine, &built, ApplyOp::Update)
        .await
        .map_err(|e| match e {
            ApplyBuiltError::Conflict => apply_conflict(),
            ApplyBuiltError::Failed(msg) => CpError::ClusterApplyFailed(msg),
        })?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `POST /engines/{id}/retire-previous-ranger-credential` — the operator's
/// confirmation that an EXTERNAL engine's plugin now runs with the credential
/// the last rotation returned: the previous one is deleted by its exact id
/// (compare-and-set on the row) and stops authenticating. Standing rule: egret
/// never deletes a credential the cluster may still hold, and for an external
/// engine only its operator can say it no longer does.
///
/// Idempotent: with no previous credential pending it answers 204 and deletes
/// nothing. A provisioned engine retires automatically after an accepted apply,
/// so it is refused (409).
#[utoipa::path(
    post,
    path = "/api/control-plane/engines/{id}/retire-previous-ranger-credential",
    tag = "control-plane-engines",
    params(("id" = Uuid, Path, description = "Engine id")),
    responses(
        (status = 204, description = "The previous Ranger plugin credential is retired (it no longer authenticates), or none was pending; the current one is untouched. A further rotation is allowed again"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:update permission", body = ErrorBody),
        (status = 404, description = "No such engine", body = ErrorBody),
        (status = 409, description = "The engine is provisioned (egret retires its previous credential itself once Kubernetes accepts an apply), or its credentials changed concurrently (re-read and retry); nothing was retired", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn retire_previous_ranger_credential(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:update").await?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    if !crate::external::is_external(&engine) {
        return Err(CpError::Conflict(format!(
            "engine {} is provisioned: egret retires its previous ranger credential itself once \
             Kubernetes accepts an apply",
            engine.name
        )));
    }
    crate::ranger_credential::RangerCredentialStore::from_state(&st)
        .retire_previous_confirmed(&engine)
        .await?;
    tracing::info!(engine = %engine.name,
        "external engine's previous ranger credential retired on the operator's confirmation");
    Ok(StatusCode::NO_CONTENT)
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
        (status = 404, description = "No such engine, or an added workspace does not exist; nothing applied", body = ErrorBody),
        (status = 409, description = "A removed workspace still has Data API endpoints, schedules or datasets on the engine's connection; nothing applied; The engine is external (`code` = `engine_is_external`): egret does not assign workspaces to it", body = EngineConflictBody),
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
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    crate::external::refuse_if_external(
        &engine,
        "assign it to a workspace (egret holds no credentials for it)",
    )?;

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
    let to_assign: Vec<Uuid> = desired.difference(&current).copied().collect();
    let to_unassign: Vec<Uuid> = current.difference(&desired).copied().collect();

    // Validate the WHOLE change before writing any of it. The assignment service
    // writes through two seams' pools, so it cannot be one transaction; every
    // refusal it could hit is therefore checked here first.
    if !st
        .engine_mgmt
        .missing_workspace_ids(&to_assign)
        .await?
        .is_empty()
    {
        return Err(CpError::NotFound);
    }
    if !to_assign.is_empty() && !engine.supports_delegation() {
        return Err(CpError::BadRequest(
            crate::assignment::NOT_DELEGATION_CAPABLE_MSG.into(),
        ));
    }
    if !to_unassign.is_empty() {
        let dependents = st
            .engine_mgmt
            .engine_dependents(id, Some(&to_unassign))
            .await?;
        if !dependents.is_empty() {
            return Err(CpError::HasDependents(dependents));
        }
    }

    let actor = p.user_id.0;
    for ws in &to_assign {
        st.assignment.assign(id, *ws, actor).await?;
    }
    for ws in &to_unassign {
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
        (status = 404, description = "No such engine or workspace", body = ErrorBody),
        (status = 409, description = "The engine is external (`code` = `engine_is_external`): egret does not assign to a workspace it", body = ErrorBody),
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
    if !st
        .engine_mgmt
        .missing_workspace_ids(&[id])
        .await?
        .is_empty()
    {
        return Err(CpError::NotFound);
    }
    let engine = st
        .engine_mgmt
        .get_engine(b.engine_id)
        .await?
        .ok_or(CpError::NotFound)?;
    crate::external::refuse_if_external(
        &engine,
        "assign it to a workspace (egret holds no credentials for it)",
    )?;
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
        (status = 404, description = "No such workspace", body = ErrorBody),
        (status = 409, description = "The workspace still has Data API endpoints, schedules or datasets on the engine's connection; nothing changed", body = EngineConflictBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn unassign_workspace_engine(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, engine_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:update").await?;
    if !st
        .engine_mgmt
        .missing_workspace_ids(&[id])
        .await?
        .is_empty()
    {
        return Err(CpError::NotFound);
    }
    // Unassigning deletes the workspace's materialized connection, which fails on
    // (endpoints, alerts) or destroys (datasets) whatever sits on it.
    let dependents = st
        .engine_mgmt
        .engine_dependents(engine_id, Some(std::slice::from_ref(&id)))
        .await?;
    if !dependents.is_empty() {
        return Err(CpError::HasDependents(dependents));
    }
    st.assignment.unassign(engine_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
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

/// Run `SHOW SCHEMAS FROM "<catalog>"` on `engine`'s Trino coordinator and
/// return the verdict as `(ok, message)`. Shared by the per-engine
/// `POST /engines/{id}/catalogs/{cid}/test` above and by the registry's
/// `POST /catalogs/{id}/test` (SP3c D4), so the two cannot drift into two
/// readings of the same coordinator reply. Errors of the query are verdicts,
/// not HTTP failures; only an unreachable coordinator (or unconfigured
/// Kubernetes) propagates as a `CpError`.
pub(crate) async fn show_schemas_verdict(
    st: &CpState,
    engine: &Engine,
    catalog_name: &str,
) -> Result<(bool, String), CpError> {
    // The chokepoint under both test routes: egret never queries the
    // coordinator of an engine it does not run.
    crate::external::refuse_if_external(engine, "run a catalog test against it")?;
    let sql = format!("SHOW SCHEMAS FROM {}", quote_ident(catalog_name));
    let (code, body) = crate::monitor::coordinator_call(
        st,
        engine,
        "POST",
        "/v1/statement",
        Some(sql.clone().into_bytes()),
    )
    .await?;
    if !(200..300).contains(&code) {
        return Ok((false, coordinator_error_message(code, &body)));
    }
    let mut v: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return Ok((false, "coordinator returned a non-JSON response".into())),
    };

    for _ in 0..=CATALOG_TEST_MAX_POLLS {
        if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
            return Ok((
                false,
                err["message"]
                    .as_str()
                    .unwrap_or("query failed")
                    .to_string(),
            ));
        }
        let state = v["stats"]["state"].as_str().unwrap_or("");
        let Some(next_uri) = v["nextUri"].as_str() else {
            return Ok((
                state == "FINISHED",
                if state == "FINISHED" {
                    // Name the check and the engine. `tcp` puts its disclaimer
                    // in the sentence and `unsupported` names the kind; the bare
                    // word "ok" left the strongest of the three the only one
                    // that did not say what it had done (SP3c review F5).
                    format!(
                        "{sql} succeeded on `{}`, so the catalog answered — credentials included.",
                        engine.name
                    )
                } else {
                    format!("query ended in state {state}")
                },
            ));
        };
        let path = next_uri_path(next_uri);
        let (code, body) = crate::monitor::coordinator_call(st, engine, "GET", &path, None).await?;
        if !(200..300).contains(&code) {
            return Ok((false, coordinator_error_message(code, &body)));
        }
        v = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(_) => return Ok((false, "coordinator returned a non-JSON response".into())),
        };
    }
    Ok((false, "timed out".into()))
}

/// The registry catalog `cid` as attached to `engine` (404 when not attached).
/// [`test_catalog`]'s only remaining caller (SP3c-2 Task 4 deleted the other
/// five legacy `/engines/{id}/catalogs*` handlers this once served too).
async fn find_attached(
    st: &CpState,
    engine: &Engine,
    cid: Uuid,
) -> Result<AttachedCatalog, CpError> {
    st.engine_mgmt
        .registry()
        .list_engine_attachments(engine.id)
        .await?
        .into_iter()
        .find(|a| a.catalog.id == cid)
        .ok_or(CpError::NotFound)
}

/// `POST /engines/{id}/catalogs/{cid}/test` payload.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct CatalogTestDto {
    ok: bool,
    message: String,
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
///
/// SP3c-2 Task 4 deleted this route's five siblings but KEPT this one: it is
/// the only route in the product that tests a *named engine's* view of a
/// catalog. The registry's `POST /catalogs/{id}/test` (`test_registry_catalog`
/// in `catalogs.rs`) picks an applied Trino attachment across ALL of a
/// catalog's engines and cannot be pointed at one, and it gates on
/// `catalog_registry:read`, and it tests an applied attachment across ALL of a
/// catalog's engines rather than one named engine. This route is the only
/// ENGINE-SCOPED test, which is why it survived SP3c-2 Task 4 while its five
/// siblings did not.
///
/// It is NOT kept for an `engine:read`-only persona, whatever an earlier draft
/// of this comment said. Task 3 re-pointed `EngineDetail.tsx`'s Catalogs tab at
/// the registry list, so that operator now sees a 403 pane and zero rows —
/// there is no `cid` for them to press Test on. `EngineDetail.tsx`'s per-row
/// Test button is a live caller (SP3c review F-3), but its user holds
/// `catalog_registry:read` like everyone else who can see a row.
#[utoipa::path(
    post,
    path = "/api/control-plane/engines/{id}/catalogs/{cid}/test",
    tag = "control-plane-engines",
    params(
        ("id" = Uuid, Path, description = "Engine id"),
        ("cid" = Uuid, Path, description = "Registry catalog id"),
    ),
    responses(
        (status = 200, description = "Test-connection result (ok=false with a message on a coordinator/query failure)", body = CatalogTestDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:read permission", body = ErrorBody),
        (status = 404, description = "No such engine or catalog (or catalog not on this engine)", body = ErrorBody),
        (status = 502, description = "Trino coordinator unreachable", body = ErrorBody),
        (status = 409, description = "The engine is external (`code` = `engine_is_external`): egret does not test a catalog on it", body = ErrorBody),
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
    crate::external::refuse_if_external(&engine, "run a catalog test against it")?;
    let cat = find_attached(&st, &engine, cid).await?.catalog;
    let (ok, message) = show_schemas_verdict(&st, &engine, &cat.name).await?;
    Ok(Json(CatalogTestDto { ok, message }))
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
        (status = 409, description = "The engine is external (`code` = `engine_is_external`): egret does not read the pods of it", body = ErrorBody),
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
    crate::external::refuse_if_external(&engine, "read its pod logs")?;
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
        (status = 409, description = "The engine is external (`code` = `engine_is_external`): egret does not read the pods of it", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn pod_logs_follow(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, pod)): Path<(Uuid, String)>,
    axum::extract::Query(q): axum::extract::Query<PodLogsFollowQuery>,
    ws: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
) -> Result<Response, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:read").await?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    // Decided before the upgrade is looked at, so the refusal is an ordinary
    // 409 a client can read rather than a failed handshake.
    crate::external::refuse_if_external(&engine, "stream its pod logs")?;
    let ws = match ws {
        Ok(ws) => ws,
        Err(rejection) => return Ok(axum::response::IntoResponse::into_response(rejection)),
    };
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
    kube: Arc<dyn egret_provisioner::KubeClient>,
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
        (status = 409, description = "The engine is external (`code` = `engine_is_external`): egret does not exec into the pods of it", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn pod_exec(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, pod)): Path<(Uuid, String)>,
    ws: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
) -> Result<Response, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:provision").await?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    crate::external::refuse_if_external(&engine, "open a shell in its pods")?;
    let ws = match ws {
        Ok(ws) => ws,
        Err(rejection) => return Ok(axum::response::IntoResponse::into_response(rejection)),
    };
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
    kube: Arc<dyn egret_provisioner::KubeClient>,
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
        list_modelled_ranger_plugins,
        create_engine,
        get_engine,
        update_engine,
        delete_engine,
        restart_engine,
        rotate_engine_secrets,
        rotate_ranger_credential,
        retire_previous_ranger_credential,
        list_engine_workspaces,
        set_engine_workspaces,
        list_workspace_engines,
        assign_workspace_engine,
        unassign_workspace_engine,
        test_catalog,
        pod_logs,
        pod_logs_follow,
        pod_exec,
    ),
    components(schemas(
        ModelledRangerPluginDto,
        CreateEngineBody,
        NewEngineCatalog,
        EngineSpecDto,
        EngineDto,
        CreateEngineResponse,
        CatalogAttachErrorDto,
        EngineDetailDto,
        EngineStatusDto,
        EnginePodDto,
        CatalogTestDto,
        PodLogsDto,
        EngineWorkspaceDto,
        SetWorkspacesBody,
        WorkspaceEngineDto,
        AssignEngineBody,
        EngineConflictBody,
        EngineDependentDto,
        EngineDependentKind,
        ErrorBody,
    )),
    tags((name = "control-plane-engines", description = "Engine provisioning: Trino-on-Kubernetes clusters, their catalogs, native query monitor, and pod logs/exec")),
)]
pub struct EngineMgmtApi;

#[cfg(test)]
mod tests {
    #[test]
    fn the_profile_conflicts_name_the_profile_the_engines_and_a_remedy_that_exists() {
        let t = profile_type_conflict("eng-1", "starrocks");
        assert!(
            t.contains("profile named eng-1 already exists with type starrocks"),
            "{t}"
        );
        assert!(
            t.contains("DELETE /api/control-plane/policies/services/eng-1"),
            "{t}"
        );
        let b = profile_bound_conflict("eng-1", &["a".into(), "b".into()]);
        assert!(
            b.contains("profile eng-1 is already bound to engine(s) a, b"),
            "{b}"
        );
        assert!(
            b.contains("PUT /api/control-plane/engines/{id} with authzServiceId"),
            "{b}"
        );
    }

    use super::*;

    fn dto_of(
        kind: &str,
        origin: &str,
        spec: serde_json::Value,
        declared: Option<&str>,
    ) -> EngineDto {
        Engine {
            id: Uuid::new_v4(),
            name: "e".into(),
            r#type: kind.into(),
            namespace: "ns".into(),
            spec,
            status: "running".into(),
            authz_service_id: None,
            delegator_secret_id: None,
            root_secret_id: None,
            tls_secret_id: None,
            coordinator_cert: None,
            tls_cert_secret_ref: None,
            internal_secret_id: None,
            monitor_secret_id: None,
            ranger_secret_id: None,
            ranger_secret_prev_id: None,
            created_at: chrono::Utc::now(),
            origin: origin.into(),
            external_host: None,
            external_port: None,
            ranger_plugin_version: declared.map(str::to_string),
        }
        .into()
    }

    #[test]
    fn a_provisioned_trino_shows_its_image_and_the_plugin_that_image_ships() {
        let d = dto_of(
            "trino",
            "provisioned",
            serde_json::json!({ "imageVersion": "483" }),
            None,
        );
        assert_eq!(d.image.as_deref(), Some("trinodb/trino:483"));
        assert_eq!(d.ranger_plugin_version.as_deref(), Some("2.8.0"));
        assert_eq!(d.ranger_plugin_version_modelled, Some(true));
        assert_eq!(d.ranger_plugin_version_note, None);
    }

    #[test]
    fn a_trino_image_override_is_what_the_dto_shows_and_judges() {
        let d = dto_of(
            "trino",
            "provisioned",
            serde_json::json!({
                "imageVersion": "483",
                "trinoConfig": { "coordinator": {}, "image": "registry.corp/trino-custom:x" }
            }),
            None,
        );
        assert_eq!(d.image.as_deref(), Some("registry.corp/trino-custom:x"));
        assert_eq!(d.ranger_plugin_version, None);
        assert_eq!(d.ranger_plugin_version_modelled, Some(false));
        let note = d.ranger_plugin_version_note.expect("a note");
        assert!(note.contains("registry.corp/trino-custom:x"), "{note}");
    }

    #[test]
    fn the_provisioner_and_the_report_agree_on_the_trino_image() {
        // Whatever the DTO reports is the image the provisioner renders: a
        // blank override falls back to `imageVersion` in BOTH, and a padded one
        // is trimmed in both.
        for (raw, expected) in [
            ("", None),
            ("   ", None),
            ("  trinodb/trino:483  ", Some("trinodb/trino:483")),
            ("registry.corp/x:1", Some("registry.corp/x:1")),
        ] {
            let spec = serde_json::json!({
                "imageVersion": "483",
                "trinoConfig": { "coordinator": {}, "image": raw }
            });
            let cfg = build_trino_config(&spec);
            assert_eq!(cfg.image.as_deref(), expected, "{raw:?}");
            let runs = cfg.image.clone().unwrap_or_else(|| {
                format!("trinodb/trino:{}", spec["imageVersion"].as_str().unwrap())
            });
            assert_eq!(
                egret_perm::stored_engine_image("trino", &spec).as_deref(),
                Some(runs.as_str()),
                "{raw:?}"
            );
        }
    }

    #[test]
    fn a_provisioned_starrocks_on_a_floating_tag_is_unmodelled_and_the_note_says_pin() {
        let d = dto_of(
            "starrocks",
            "provisioned",
            serde_json::json!({ "starrocksConfig": { "feImage": "starrocks/fe-ubuntu:4.1-latest" } }),
            None,
        );
        assert_eq!(d.image.as_deref(), Some("starrocks/fe-ubuntu:4.1-latest"));
        assert_eq!(d.ranger_plugin_version, None);
        assert_eq!(d.ranger_plugin_version_modelled, Some(false));
        let note = d.ranger_plugin_version_note.expect("a note");
        assert!(note.contains("pin"), "{note}");
        assert!(note.contains("4.1-latest"), "{note}");
    }

    #[test]
    fn a_provisioned_engine_with_no_image_is_unmodelled_not_silently_fine() {
        let d = dto_of("trino", "provisioned", serde_json::json!({}), None);
        assert_eq!(d.image, None);
        assert_eq!(d.ranger_plugin_version_modelled, Some(false));
        assert!(d.ranger_plugin_version_note.is_some());
    }

    /// The fix for an engine with no image is to pin an IMAGE, so the note
    /// lists the known images of the engine's own type (as
    /// `plugin_version_for_image` does), not (type, plugin) pairs.
    #[test]
    fn the_no_image_note_lists_the_known_images_of_its_type() {
        for (ty, spec) in [
            ("trino", serde_json::json!({})),
            ("starrocks", serde_json::json!({ "starrocksConfig": {} })),
        ] {
            let note = dto_of(ty, "provisioned", spec, None)
                .ranger_plugin_version_note
                .expect("a note");
            for k in egret_perm::KNOWN_ENGINE_IMAGES.iter() {
                let image = format!("{}:{}", k.repository, k.tag);
                assert_eq!(
                    note.contains(&image),
                    k.engine_type == ty,
                    "{ty}: {image} in {note}"
                );
            }
            assert!(!note.contains("@2."), "{ty}: no pairs: {note}");
        }
    }

    #[test]
    fn an_external_engine_keeps_its_declared_version_and_has_no_image() {
        let d = dto_of(
            "starrocks",
            "external",
            serde_json::json!({ "starrocksConfig": { "feImage": "starrocks/fe-ubuntu:4.1.6" } }),
            Some("2.9.0"),
        );
        assert_eq!(d.image, None);
        assert_eq!(d.ranger_plugin_version.as_deref(), Some("2.9.0"));
        assert_eq!(d.ranger_plugin_version_modelled, Some(true));
        assert_eq!(d.ranger_plugin_version_note, None);
        let d = dto_of("trino", "external", serde_json::json!({}), Some("2.9.0"));
        assert_eq!(d.ranger_plugin_version_modelled, Some(false));
        assert!(d.ranger_plugin_version_note.is_some());
    }

    #[test]
    fn per_role_pod_shape_parses_and_legacy_falls_back_to_both_roles() {
        // New per-role shape: each role carries its own init containers.
        let tc = serde_json::json!({
            "coordinator": { "initContainers": [{ "name": "coord-init", "image": "busybox" }] },
            "worker": { "initContainers": [{ "name": "worker-init", "image": "busybox" }] },
        });
        let cfg = trino_config_from_json(&tc);
        assert_eq!(cfg.coordinator.init_containers[0].name, "coord-init");
        assert_eq!(cfg.worker.base.init_containers[0].name, "worker-init");

        // Legacy shape: a top-level `initContainers` applies to BOTH roles.
        let legacy = serde_json::json!({
            "initContainers": [{ "name": "legacy-init", "image": "busybox" }],
            "shareProcessNamespace": true,
        });
        let cfg = trino_config_from_json(&legacy);
        assert_eq!(cfg.coordinator.init_containers[0].name, "legacy-init");
        assert_eq!(cfg.worker.base.init_containers[0].name, "legacy-init");
        assert_eq!(cfg.coordinator.share_process_namespace, Some(true));
        assert_eq!(cfg.worker.base.share_process_namespace, Some(true));
    }

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
    fn legacy_monitor_password_is_derived_from_id() {
        assert_eq!(
            legacy_monitor_password(Uuid::nil()),
            "egret-monitor-00000000-0000-0000-0000-000000000000"
        );
    }

    #[test]
    fn inject_monitor_user_adds_egret_monitor_when_auth_on() {
        // An engine WITH auth (one human password user).
        let mut auth = Some(AuthReq {
            password_users: vec![PasswordEntryReq {
                username: "bob".into(),
                bcrypt_hash: "$2y$hash".into(),
            }],
            jwt: None,
        });
        inject_monitor_user(&mut auth, "pw-123").unwrap();

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
        assert!(bcrypt::verify("pw-123", &hash).unwrap());
    }

    #[test]
    fn inject_monitor_user_noop_when_no_auth() {
        // A no-auth engine must NOT gain an auth block (egret-monitor not forced on).
        let mut auth: Option<AuthReq> = None;
        inject_monitor_user(&mut auth, "pw").unwrap();
        assert!(auth.is_none());
    }

    /// Fix #3c: enabling the engine `jwt` block with no JWKS anywhere (platform OIDC
    /// unconfigured AND no per-engine `jwksUri`) must be REJECTED at the API boundary, not
    /// silently provision a coordinator that crashes at boot. Self-skips without
    /// `DATABASE_URL` ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS (mirrors the
    /// other `test_state` tests).
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

    /// Task 3: `resolve_auth`'s jwt block defaults `required_audience` to
    /// `"engine"` — the SAME audience egret's machine-IdP mints into every
    /// engine token and StarRocks' JWT integration requires unconditionally —
    /// when the admin doesn't set `trinoConfig.auth.jwt.requiredAudience`. An
    /// explicit per-engine override is still honored.
    #[tokio::test]
    async fn resolve_auth_defaults_jwt_audience_to_engine() {
        let Some(mut st) = test_state().await else {
            eprintln!(
                "skipping resolve_auth_defaults_jwt_audience_to_engine: DATABASE_URL not set"
            );
            return;
        };
        st.engine_jwks_uri = "https://idp/jwks".into();

        // No requiredAudience in the request ⇒ defaults to "engine".
        let tc = serde_json::json!({ "auth": { "jwt": {} } });
        let jwt = resolve_auth(&st, &tc)
            .await
            .expect("jwt accepted")
            .and_then(|a| a.jwt)
            .expect("jwt present");
        assert_eq!(jwt.required_audience.as_deref(), Some("engine"));

        // Explicit override is preserved, not clobbered by the default.
        let tc_override =
            serde_json::json!({ "auth": { "jwt": { "requiredAudience": "custom-aud" } } });
        let jwt2 = resolve_auth(&st, &tc_override)
            .await
            .expect("jwt accepted")
            .and_then(|a| a.jwt)
            .expect("jwt present");
        assert_eq!(jwt2.required_audience.as_deref(), Some("custom-aud"));
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

    /// The FE metadata PVC knobs follow the same omit=preserve rule as every
    /// other field, and an engine created before the surface existed stores
    /// `null` for both — which the provisioner reads as "default StorageClass
    /// + default size". That says nothing about whether such an engine gets a
    ///   PVC: it does NOT, because it carries no `feMetaPersistent` marker (see
    ///   `build_starrocks_spec_marks_only_engines_it_creates`). These two fields
    ///   only choose the class and size of a volume the marker decides to render.
    #[test]
    fn build_starrocks_spec_carries_fe_meta_storage_knobs() {
        let created = build_starrocks_spec(&StarRocksConfigBody::default(), None);
        let ccfg = &created["starrocksConfig"];
        assert!(ccfg["feMetaStorageClass"].is_null());
        assert!(ccfg["feMetaStorageSize"].is_null());

        let body = StarRocksConfigBody {
            fe_meta_storage_class: Some("fast-ssd".into()),
            fe_meta_storage_size: Some("50Gi".into()),
            ..Default::default()
        };
        let set = build_starrocks_spec(&body, None);
        assert_eq!(set["starrocksConfig"]["feMetaStorageClass"], "fast-ssd");
        assert_eq!(set["starrocksConfig"]["feMetaStorageSize"], "50Gi");

        // An unrelated edit preserves them.
        let kept = build_starrocks_spec(
            &StarRocksConfigBody {
                fe_replicas: Some(3),
                ..Default::default()
            },
            Some(&set),
        );
        assert_eq!(kept["starrocksConfig"]["feMetaStorageClass"], "fast-ssd");
        assert_eq!(kept["starrocksConfig"]["feMetaStorageSize"], "50Gi");

        // And an update CANNOT change them, body or no body. The handler refuses
        // such a PUT (`validate_sr_fe_meta_immutable`); this pins that the
        // builder itself is incapable of it, so a caller that skipped the check
        // still could not rewrite an immutable `volumeClaimTemplates` field.
        let forced = build_starrocks_spec(
            &StarRocksConfigBody {
                fe_meta_storage_class: Some("slow-hdd".into()),
                fe_meta_storage_size: Some("500Gi".into()),
                ..Default::default()
            },
            Some(&set),
        );
        assert_eq!(forced["starrocksConfig"]["feMetaStorageClass"], "fast-ssd");
        assert_eq!(forced["starrocksConfig"]["feMetaStorageSize"], "50Gi");
    }

    /// The FE-metadata persistence MARKER is stamped by create and only by
    /// create. `volumeClaimTemplates` is immutable on a live StatefulSet, so an
    /// engine that already exists must never acquire the marker — an update can
    /// only preserve what is stored, and an update of a legacy engine leaves it
    /// `false`.
    #[test]
    fn build_starrocks_spec_marks_only_engines_it_creates() {
        // CREATE ⇒ marked.
        let created = build_starrocks_spec(&StarRocksConfigBody::default(), None);
        assert_eq!(created["starrocksConfig"]["feMetaPersistent"], true);

        // UPDATE of a marked engine ⇒ still marked.
        let kept = build_starrocks_spec(
            &StarRocksConfigBody {
                fe_replicas: Some(3),
                ..Default::default()
            },
            Some(&created),
        );
        assert_eq!(kept["starrocksConfig"]["feMetaPersistent"], true);

        // UPDATE of a LEGACY engine — a stored spec written before the marker
        // existed ⇒ NOT marked. This is the case the live gate falsified the
        // plan on.
        let legacy = serde_json::json!({
            "imageVersion": "3.3-latest",
            "starrocksConfig": { "feImage": "starrocks/fe-ubuntu:3.3-latest", "feReplicas": 1 },
        });
        let updated = build_starrocks_spec(
            &StarRocksConfigBody {
                fe_replicas: Some(2),
                ..Default::default()
            },
            Some(&legacy),
        );
        assert_eq!(updated["starrocksConfig"]["feMetaPersistent"], false);

        // And a stored spec with NO `starrocksConfig` object at all is still an
        // UPDATE, not a create: `existing_cfg` is `None` for both, so matching
        // on it instead of on `existing` would mark a live engine.
        let bare = build_starrocks_spec(
            &StarRocksConfigBody::default(),
            Some(&serde_json::json!({})),
        );
        assert_eq!(bare["starrocksConfig"]["feMetaPersistent"], false);
    }

    /// A MARKED engine's update may not change its FE metadata volume. This is
    /// the test that matters: `feMetaStorageClass` / `feMetaStorageSize` render
    /// inside the FE StatefulSet's `volumeClaimTemplates`, which Kubernetes
    /// makes immutable once the StatefulSet exists, so a PUT that changed them
    /// would park the whole `StarRocksCluster` at `phase=failed`. The same
    /// assertion on an UNMARKED engine would pass against the bug, which is why
    /// the marked spec is the subject here.
    #[test]
    fn update_refuses_fe_meta_storage_on_a_marked_engine() {
        let marked = build_starrocks_spec(&StarRocksConfigBody::default(), None);
        assert_eq!(marked["starrocksConfig"]["feMetaPersistent"], true);

        for body in [
            StarRocksConfigBody {
                fe_meta_storage_size: Some("50Gi".into()),
                ..Default::default()
            },
            StarRocksConfigBody {
                fe_meta_storage_class: Some("fast-ssd".into()),
                ..Default::default()
            },
        ] {
            let err = validate_sr_fe_meta_immutable(&body, &marked)
                .expect_err("a marked engine must refuse an FE-meta volume change");
            let msg = format!("{err:?}");
            assert!(
                msg.contains("not accepted on update"),
                "refuse, never accept-and-drop: {msg}"
            );
            assert!(
                msg.contains("volumeClaimTemplates") && msg.contains("immutable"),
                "the message must say WHY: {msg}"
            );
            assert!(
                msg.contains("--cascade=orphan") && msg.contains("DESTROYS"),
                "the message must tell the operator what to do instead, and its cost: {msg}"
            );
            // A wrapped literal whose line continuations were dropped reads as
            // one line with runs of indentation in it. Operators see this text.
            assert!(
                !msg.contains("  "),
                "the message must not carry source indentation: {msg}"
            );
        }

        // An UNMARKED engine is refused too — its stored value would never be
        // rendered — but for its own reason, and it must NOT claim a wedge.
        let legacy = serde_json::json!({
            "starrocksConfig": { "feImage": "starrocks/fe-ubuntu:3.3-latest" },
        });
        let err = validate_sr_fe_meta_immutable(
            &StarRocksConfigBody {
                fe_meta_storage_size: Some("50Gi".into()),
                ..Default::default()
            },
            &legacy,
        )
        .expect_err("an unmarked engine has no PVC to configure");
        let msg = format!("{err:?}");
        assert!(msg.contains("not accepted on update"), "{msg}");
        assert!(msg.contains("no FE metadata"), "{msg}");
        assert!(!msg.contains("  "), "source indentation leaked: {msg}");
        assert!(
            !msg.contains("cascade=orphan"),
            "an engine with no PVC must not be told to recreate a claim template: {msg}"
        );

        // Absent or BLANK is the omit=preserve case and changes nothing, so it
        // passes on both — refusing it would break an idempotent GET→PUT.
        for spec in [&marked, &legacy] {
            validate_sr_fe_meta_immutable(&StarRocksConfigBody::default(), spec)
                .expect("an omitted field is preserve, not a change");
            validate_sr_fe_meta_immutable(
                &StarRocksConfigBody {
                    fe_meta_storage_class: Some("   ".into()),
                    fe_meta_storage_size: Some("".into()),
                    ..Default::default()
                },
                spec,
            )
            .expect("a blank field is preserve, not a change");
        }
    }

    /// Create rejects a class or size the operator would read as "render no
    /// PersistentVolumeClaim", which on a live engine would REMOVE the claim
    /// template — the same immutable-field wedge from the other direction.
    #[test]
    fn create_rejects_fe_meta_storage_that_disables_the_pvc() {
        for class in ["emptyDir", "hostPath", "EMPTYDIR", "HostPath", " emptydir "] {
            let err = validate_sr_fe_meta_storage(&StarRocksConfigBody {
                fe_meta_storage_class: Some(class.into()),
                ..Default::default()
            })
            .expect_err(class);
            let msg = format!("{err:?}");
            assert!(msg.contains("is not a StorageClass"), "{class}: {msg}");
            assert!(!msg.contains("  "), "source indentation leaked: {msg}");
        }
        // A real class, and an absent/blank one, are fine.
        for class in [None, Some("".to_string()), Some("fast-ssd".to_string())] {
            validate_sr_fe_meta_storage(&StarRocksConfigBody {
                fe_meta_storage_class: class.clone(),
                ..Default::default()
            })
            .unwrap_or_else(|e| panic!("{class:?} should pass: {e:?}"));
        }

        // Zero sizes render no claim; malformed ones are refused by the CRD at
        // apply time, minutes later, with nothing readable to point at.
        for size in [
            "0", "0Gi", "0.0Gi", "00Gi", "20", "20GB", "20G", "20Ki", "1.2.3Gi", "Gi", "big",
            "-1Gi",
        ] {
            let err = validate_sr_fe_meta_storage(&StarRocksConfigBody {
                fe_meta_storage_size: Some(size.into()),
                ..Default::default()
            })
            .expect_err(size);
            let msg = format!("{err:?}");
            assert!(msg.contains("positive binary quantity"), "{size}: {msg}");
            assert!(!msg.contains("  "), "source indentation leaked: {msg}");
        }
        for size in ["20Gi", "1.5Ti", ".5Gi", "512Mi", "1Pi", "1Ei", " 50Gi "] {
            validate_sr_fe_meta_storage(&StarRocksConfigBody {
                fe_meta_storage_size: Some(size.into()),
                ..Default::default()
            })
            .unwrap_or_else(|e| panic!("{size} should pass: {e:?}"));
        }
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
        assert_eq!(ccfg["feImage"], DEFAULT_ENGINE_IMAGES.starrocks_fe_image);
        assert_eq!(ccfg["cnImage"], DEFAULT_ENGINE_IMAGES.starrocks_cn_image);
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

    fn dummy_engine() -> Engine {
        Engine {
            id: Uuid::new_v4(),
            name: "e".into(),
            r#type: "trino".into(),
            namespace: "egret-engines".into(),
            spec: serde_json::json!({}),
            status: "pending".into(),
            authz_service_id: None,
            delegator_secret_id: None,
            root_secret_id: None,
            tls_secret_id: None,
            coordinator_cert: None,
            tls_cert_secret_ref: None,
            internal_secret_id: None,
            monitor_secret_id: None,
            ranger_secret_id: None,
            ranger_secret_prev_id: None,
            created_at: chrono::Utc::now(),
            origin: "provisioned".into(),
            external_host: None,
            external_port: None,
            ranger_plugin_version: None,
        }
    }

    /// Build a real `EngineMgmtState` against the local dev Postgres (mirrors
    /// the self-skip pattern used by `tests/routes_api.rs`); `None` when
    /// `DATABASE_URL` isn't set so this suite doesn't fail in environments
    /// without a database.
    async fn test_state() -> Option<CpState> {
        let dsn = egret_test_db::url().await?;
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
            users: egret_auth::users::UserRepo::new(db.clone()),
            roles: egret_auth::roles::RoleRepo::new(db.clone(), egret_events::EventBus::new()),
            pdp: std::sync::Arc::new(AllowAllPdp),
            engine_provisioner: None,
            kube: None,
            engine_egret_url: "http://egret:8080".into(),
            engine_jwks_uri: "https://idp/jwks".into(),
            engine_oidc_issuer: "https://idp/realms/egret".into(),
            ranger_enabled: false,
            ranger_surface_mode: crate::state::RangerSurfaceMode::Required,
            sr_jdbc_drivers: egret_catalog_registry::JdbcDrivers::default(),
            starrocks_ddl: std::sync::Arc::new(crate::starrocks_ddl::FakeStarRocksDdl::default()),
            perm_compiler: None,
            catalog_assignment_compiler: None,
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
                endpoint: Some("http://rustfs:9000".into()),
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

    /// P2: compensation deletes the engine row, every secret egret minted for it
    /// (including pending internal secrets already recorded), and the catalog
    /// client — and never the admin-provided coordinator TLS secret.
    #[tokio::test]
    async fn compensate_failed_create_removes_everything_it_minted() {
        let Some(st) = test_state().await else {
            eprintln!("skipping compensate_failed_create_removes_everything_it_minted: DATABASE_URL not set");
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (wh, _secret) = seed_sr_warehouse(&st, &db).await;
        let mut tls = std::collections::BTreeMap::new();
        tls.insert("tls.crt".to_string(), "CERTPEM".to_string());
        tls.insert("tls.key".to_string(), "KEYPEM".to_string());
        let admin_tls = st
            .secrets
            .create_typed(
                None,
                &format!("company-tls-{}", Uuid::new_v4().simple()),
                None,
                egret_secrets::secret_type::TLS,
                &tls,
            )
            .await
            .expect("admin tls secret");
        let engine = st
            .engine_mgmt
            .create_engine(
                &format!("trino-{}", Uuid::new_v4().simple()),
                "trino",
                "egret-engines",
                &serde_json::json!({ "trinoConfig": { "delegationMode": "impersonate", "auth": {} } }),
                Some(admin_tls),
            )
            .await
            .expect("engine row");
        let (delegator_id, _) = ensure_delegator_secret(&st, &engine)
            .await
            .expect("delegator");
        let engine = st.engine_mgmt.get_engine(engine.id).await.unwrap().unwrap();
        let internal = resolve_internal_secrets(&st, &engine, false)
            .await
            .expect("internal");
        commit_internal_secrets(&st, &engine, &internal, Supersede::Keep)
            .await
            .expect("commit");
        let role_id = ensure_engine_role(&st, wh.id).await.expect("role");
        let client = st
            .catalog
            .create_client(NewClient {
                name: format!("engine-{}", engine.name),
                consumer_type: "engine".into(),
                workspace_id: wh.workspace_id,
                warehouse_id: wh.id,
                catalog_role_id: role_id,
            })
            .await
            .expect("client");

        compensate_failed_create(
            &st,
            CreateUndo {
                client_id: Some(client.id),
                engine_id: Some(engine.id),
                ..CreateUndo::default()
            },
        )
        .await;

        assert!(st
            .engine_mgmt
            .get_engine(engine.id)
            .await
            .unwrap()
            .is_none());
        for id in [delegator_id, internal.internal_id, internal.monitor_id] {
            assert!(
                matches!(
                    st.secrets.resolve_plaintext(None, id).await,
                    Err(egret_secrets::SecretsError::NotFound)
                ),
                "secret {id} must be deleted"
            );
        }
        assert!(
            st.secrets.resolve_typed(None, admin_tls).await.is_ok(),
            "an admin-provided TLS secret is never deleted"
        );
        assert!(!st
            .catalog
            .list_clients(wh.id)
            .await
            .unwrap()
            .iter()
            .any(|c| c.id == client.id));
    }

    #[test]
    fn engine_owned_secret_ids_excludes_the_admin_tls_reference() {
        let mut e = dummy_engine();
        let ids: Vec<Uuid> = (0..6).map(|_| Uuid::new_v4()).collect();
        e.delegator_secret_id = Some(ids[0]);
        e.tls_secret_id = Some(ids[1]);
        e.root_secret_id = Some(ids[2]);
        e.internal_secret_id = Some(ids[3]);
        e.monitor_secret_id = Some(ids[4]);
        e.tls_cert_secret_ref = Some(ids[5]);
        assert_eq!(engine_owned_secret_ids(&e), ids[..5].to_vec());
    }

    /// I1 fix: when the cluster delete `compensate_failed_create` itself
    /// attempts ALSO fails, the row must be kept (marked `failed`, not
    /// deleted) and none of its secrets removed — the apply may already have
    /// written live cluster objects from them that this call could not clean
    /// up, and deleting the row would strand those objects with no way back
    /// in through the API. The create's ORIGINAL error (the injected
    /// provisioning failure) must still be what the caller sees, not a
    /// compensation-failure error.
    #[tokio::test]
    async fn create_keeps_the_row_failed_when_compensations_cluster_delete_also_fails() {
        let Some(mut st) = test_state().await else {
            eprintln!(
                "skipping create_keeps_the_row_failed_when_compensations_cluster_delete_also_fails: DATABASE_URL not set"
            );
            return;
        };
        st.engine_provisioner = Some(std::sync::Arc::new(FailingProvisionAndDeleteProvisioner));
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (wh, _s) = seed_sr_warehouse(&st, &db).await;
        // Engines only provision against a lakekeeper-backed warehouse.
        egret_catalog_engine::CatalogStore::new(db.clone())
            .set_lakekeeper_warehouse_id(wh.id, "lk-test")
            .await
            .expect("mark warehouse provisioned");

        let name = format!("trino-{}", Uuid::new_v4().simple());
        let body = CreateEngineBody {
            name: name.clone(),
            warehouse_id: Some(wh.id),
            catalogs: Vec::new(),
            image_version: None,
            coordinator_replicas: 1,
            worker_replicas: 1,
            cpu_min: "1".into(),
            cpu_max: "1".into(),
            memory: "1Gi".into(),
            // Impersonate delegation so the create also mints a delegator +
            // TLS secret, not just the internal/monitor pair — exercises the
            // full `engine_owned_secret_ids` set.
            trino_config: Some(serde_json::json!({ "delegationMode": "impersonate", "auth": {} })),
            authz_service_id: None,
            kind: "trino".into(),
            starrocks: None,
            tls_cert_secret_ref: None,
        };
        let principal = egret_core::context::Principal {
            user_id: egret_core::ids::UserId::new(),
            email: "admin@egret.local".into(),
            platform_roles: vec!["platform_admin".into()],
            permissions: vec![],
            group_ids: vec![],
            impersonator: None,
        };

        let msg = match create_engine(State(st.clone()), Auth(principal), Json(body)).await {
            Err(CpError::BadRequest(msg)) => msg,
            Err(other) => panic!("expected BadRequest, got {other:?}"),
            Ok(_) => panic!("expected the injected provision failure to surface"),
        };
        assert!(
            msg.contains("injected provision failure"),
            "the response must still be the create's ORIGINAL error, got: {msg}"
        );

        let engine = st
            .engine_mgmt
            .list_engines()
            .await
            .unwrap()
            .into_iter()
            .find(|e| e.name == name)
            .expect("row kept, not deleted");
        assert_eq!(
            engine.status,
            crate::engine_status::FAILED,
            "row marked failed, not left provisioning"
        );
        let owned = engine_owned_secret_ids(&engine);
        assert!(!owned.is_empty(), "delegation should have minted secrets");
        for id in owned {
            assert!(
                st.secrets.resolve_typed(None, id).await.is_ok(),
                "secret {id} must still resolve (compensation's own cluster delete failed)"
            );
        }
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
        let dsn = egret_test_db::url().await.unwrap();
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
        assert_eq!(sr.storage_endpoint, "http://rustfs:9000");
        assert_eq!(sr.storage_access_key, "egret-ak");
        assert_eq!(sr.storage_secret_key, secret_plain);
        assert_eq!(
            sr.storage_path,
            format!("warehouse/starrocks/{}", engine.name)
        );
        // Delegation block present in the stored spec ⇒ delegation + bootstrap.
        assert!(sr.delegation.is_some());
        assert!(boot.is_some());
        // Task 3: unified STS audience — every engine's JWT integration must
        // require `aud:"engine"` (egret's machine-IdP-minted claim), not the
        // old per-kind literal `"starrocks"`.
        assert_eq!(sr.delegation.as_ref().unwrap().audience, "engine");
        assert_eq!(boot.as_ref().unwrap().audience, "engine");

        // (b) A NO-DELEGATION engine row: images stored, delegation absent.
        let spec2 = serde_json::json!({
            "starrocksConfig": {
                "feImage": DEFAULT_ENGINE_IMAGES.starrocks_fe_image,
                "cnImage": DEFAULT_ENGINE_IMAGES.starrocks_cn_image,
                "feReplicas": 2,
            },
        });
        let engine2 = st
            .engine_mgmt
            .create_engine(
                &format!("sr-{}", Uuid::new_v4().simple()),
                "starrocks",
                "egret-engines",
                &spec2,
                None,
            )
            .await
            .expect("create engine row 2");
        let (req2, boot2) = build_starrocks_provision_req(&st, &engine2, &wh)
            .await
            .expect("build_starrocks_provision_req (no delegation)");
        let sr2 = req2.starrocks.as_ref().expect("starrocks config present");
        assert_eq!(sr2.fe_image, DEFAULT_ENGINE_IMAGES.starrocks_fe_image);
        assert_eq!(sr2.cn_image, DEFAULT_ENGINE_IMAGES.starrocks_cn_image);
        assert_eq!(sr2.fe_replicas, 2);
        assert_eq!(sr2.cn_replicas, 1);
        assert!(sr2.delegation.is_none());
        assert!(boot2.is_none());
    }

    /// SP4a Task 3b — the defect this closes: a bound Ranger profile was only
    /// ever enforced if its NAME happened to equal the engine's, because both
    /// `build_provision_req` and `build_starrocks_provision_req` derived the
    /// Ranger service name as `authz_service_id.map(|_| engine.name.clone())`,
    /// discarding the bound profile's own name. A test where the two names
    /// coincide (the auto-bind shape every OTHER test in this file uses)
    /// passes against the bug and proves nothing — this engine and its bound
    /// profile are deliberately named DIFFERENTLY.
    #[tokio::test]
    async fn build_starrocks_provision_req_names_ranger_binding_after_the_bound_profile_not_the_engine(
    ) {
        let Some(st) = test_state().await else {
            eprintln!(
                "skipping build_starrocks_provision_req_names_ranger_binding_after_the_bound_profile_not_the_engine: DATABASE_URL not set"
            );
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (wh, _secret) = seed_sr_warehouse(&st, &db).await;

        let engine_name = format!("sr-engine-{}", Uuid::new_v4().simple());
        let profile_name = format!("sr-profile-{}", Uuid::new_v4().simple());
        assert_ne!(engine_name, profile_name, "test requires DIFFERING names");

        // The service-def type is irrelevant to what this test asserts (name
        // resolution, not enforcement), so seed only the "trino" def rather
        // than reaching for `seed_starrocks` (not exposed on `PolicyPort`).
        st.authz
            .ensure_trino_def()
            .await
            .expect("seed trino service-def");
        let svc = st
            .authz
            .create_service(&profile_name, "trino", &serde_json::json!({}))
            .await
            .expect("create policy profile");

        let engine = st
            .engine_mgmt
            .create_engine(
                &engine_name,
                "starrocks",
                "egret-engines",
                &serde_json::json!({ "starrocksConfig": { "feImage": "starrocks/fe-ubuntu:4.1.6", "cnImage": "starrocks/cn-ubuntu:4.1.6", "feReplicas": 1 } }),
                None,
            )
            .await
            .expect("create engine row");
        st.engine_mgmt
            .set_engine_authz_service(engine.id, Some(svc.id))
            .await
            .expect("bind engine to the differently-named profile");
        let engine = st
            .engine_mgmt
            .get_engine(engine.id)
            .await
            .unwrap()
            .expect("reloaded engine");

        let (req, _boot) = build_starrocks_provision_req(&st, &engine, &wh)
            .await
            .expect("build_starrocks_provision_req");
        let ranger = req
            .ranger
            .as_ref()
            .expect("bound engine must carry a RangerBinding");
        assert_eq!(
            ranger.service_name, profile_name,
            "must poll the BOUND profile's name, not the engine's"
        );
        assert_ne!(ranger.service_name, engine.name);
    }

    /// Trino counterpart of the StarRocks test above, through
    /// `build_provision_req` (Task 3's other call site, `:583`).
    #[tokio::test]
    async fn build_provision_req_names_ranger_binding_after_the_bound_profile_not_the_engine() {
        let Some(st) = test_state().await else {
            eprintln!(
                "skipping build_provision_req_names_ranger_binding_after_the_bound_profile_not_the_engine: DATABASE_URL not set"
            );
            return;
        };
        let engine_name = format!("trino-engine-{}", Uuid::new_v4().simple());
        let profile_name = format!("trino-profile-{}", Uuid::new_v4().simple());
        assert_ne!(engine_name, profile_name, "test requires DIFFERING names");

        st.authz
            .ensure_trino_def()
            .await
            .expect("seed trino service-def");
        let svc = st
            .authz
            .create_service(&profile_name, "trino", &serde_json::json!({}))
            .await
            .expect("create policy profile");

        let engine = st
            .engine_mgmt
            .create_engine(
                &engine_name,
                "trino",
                "egret-engines",
                &serde_json::json!({ "imageVersion": "483", "trinoConfig": {} }),
                None,
            )
            .await
            .expect("create engine row");
        st.engine_mgmt
            .set_engine_authz_service(engine.id, Some(svc.id))
            .await
            .expect("bind engine to the differently-named profile");
        let engine = st
            .engine_mgmt
            .get_engine(engine.id)
            .await
            .unwrap()
            .expect("reloaded engine");

        let built = build_provision_req(&st, &engine)
            .await
            .expect("build_provision_req");
        let ranger = built
            .req
            .ranger
            .as_ref()
            .expect("bound engine must carry a RangerBinding");
        assert_eq!(
            ranger.service_name, profile_name,
            "must poll the BOUND profile's name, not the engine's"
        );
        assert_ne!(ranger.service_name, engine.name);
    }

    /// A Trino engine row bound to a fresh Trino profile, with `trinoConfig`
    /// as given. Returns the reloaded engine and the profile's id and name.
    async fn bound_trino_engine(
        st: &CpState,
        trino_config: serde_json::Value,
    ) -> (Engine, i64, String) {
        let engine_name = format!("trino-ub-{}", Uuid::new_v4().simple());
        let profile_name = format!("ub_{}", Uuid::new_v4().simple());
        st.authz
            .ensure_trino_def()
            .await
            .expect("seed trino service-def");
        let svc = st
            .authz
            .create_service(&profile_name, "trino", &serde_json::json!({}))
            .await
            .expect("create policy profile");
        let engine = st
            .engine_mgmt
            .create_engine(
                &engine_name,
                "trino",
                "egret-engines",
                &serde_json::json!({ "imageVersion": "483", "trinoConfig": trino_config }),
                None,
            )
            .await
            .expect("create engine row");
        st.engine_mgmt
            .set_engine_authz_service(engine.id, Some(svc.id))
            .await
            .expect("bind engine");
        let engine = st
            .engine_mgmt
            .get_engine(engine.id)
            .await
            .unwrap()
            .expect("reloaded engine");
        (engine, svc.id, profile_name)
    }

    /// `(name, body, managed_by)` of every policy stored under `service_id`.
    async fn stored_policies(service_id: i64) -> Vec<(String, serde_json::Value, String)> {
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 2)
            .await
            .expect("connect");
        sqlx::query_as(
            "SELECT name, policy_jsonb, managed_by FROM policy.authz_policies \
             WHERE service_id = $1 ORDER BY id",
        )
        .bind(service_id)
        .fetch_all(&db.pool)
        .await
        .expect("read policies")
    }

    /// Every policy of the per-user baseline is stored exactly once, with the
    /// producer's body, owned by egret's plumbing.
    fn assert_user_baseline(rows: &[(String, serde_json::Value, String)], profile_name: &str) {
        use crate::delegator_policy as dp;
        for (name, body) in dp::user_baseline_policies(profile_name) {
            let hits: Vec<_> = rows.iter().filter(|(n, _, _)| n == name).collect();
            assert_eq!(
                hits.len(),
                1,
                "{name}: expected exactly one row, got {rows:#?}"
            );
            assert_eq!(
                hits[0].1, body,
                "{name}: stored body differs from the producer's"
            );
            assert_eq!(
                hits[0].2, "infra",
                "{name}: must be owned by egret's plumbing"
            );
        }
    }

    /// Task 7 (live finding B-F1): a JWT-delegated Trino engine's apply seeds
    /// the per-user baseline Trino 483 checks on every session. Before this,
    /// only impersonate engines got any of it, so every JWT query failed
    /// `Principal u cannot become user u`. The delegator's own grant is NOT
    /// seeded here: a JWT engine has no delegator.
    #[tokio::test]
    async fn a_jwt_engine_apply_seeds_the_user_baseline_and_no_delegator_grant() {
        let Some(st) = test_state().await else {
            eprintln!("skipping a_jwt_engine_apply_seeds_the_user_baseline_and_no_delegator_grant: DATABASE_URL not set");
            return;
        };
        let (engine, sid, profile) = bound_trino_engine(
            &st,
            serde_json::json!({ "delegationMode": "jwt", "auth": { "jwt": {} } }),
        )
        .await;
        assert!(engine.is_jwt_delegation() && engine.supports_delegation());

        build_provision_req(&st, &engine)
            .await
            .expect("build_provision_req");

        let rows = stored_policies(sid).await;
        assert_user_baseline(&rows, &profile);
        assert!(
            !rows
                .iter()
                .any(|(n, _, _)| n == crate::delegator_policy::IMPERSONATE_POLICY_NAME),
            "a JWT engine has no delegator: {rows:#?}"
        );
    }

    /// An impersonate engine with auth keeps its delegator grant
    /// (impersonate-only, unchanged) and gets the same per-user baseline.
    #[tokio::test]
    async fn an_impersonate_engine_apply_seeds_the_delegator_grant_and_the_user_baseline() {
        let Some(st) = test_state().await else {
            eprintln!("skipping an_impersonate_engine_apply_seeds_the_delegator_grant_and_the_user_baseline: DATABASE_URL not set");
            return;
        };
        let (engine, sid, profile) = bound_trino_engine(
            &st,
            serde_json::json!({ "delegationMode": "impersonate", "auth": {} }),
        )
        .await;
        assert!(engine.is_impersonate_delegation());

        build_provision_req(&st, &engine)
            .await
            .expect("build_provision_req");

        let rows = stored_policies(sid).await;
        assert_user_baseline(&rows, &profile);
        let delegator: Vec<_> = rows
            .iter()
            .filter(|(n, _, _)| n == crate::delegator_policy::IMPERSONATE_POLICY_NAME)
            .collect();
        assert_eq!(delegator.len(), 1, "{rows:#?}");
        assert_eq!(
            delegator[0].1,
            crate::delegator_policy::delegator_impersonate_policy(&profile)
        );
        assert_eq!(delegator[0].2, "infra");
    }

    /// Applying twice leaves one row per name: the seed is idempotent.
    #[tokio::test]
    async fn applying_twice_seeds_no_duplicate_baseline_policies() {
        let Some(st) = test_state().await else {
            eprintln!("skipping applying_twice_seeds_no_duplicate_baseline_policies: DATABASE_URL not set");
            return;
        };
        let (engine, sid, profile) = bound_trino_engine(
            &st,
            serde_json::json!({ "delegationMode": "jwt", "auth": { "jwt": {} } }),
        )
        .await;
        for _ in 0..2 {
            build_provision_req(&st, &engine)
                .await
                .expect("build_provision_req");
        }
        let rows = stored_policies(sid).await;
        assert_user_baseline(&rows, &profile);
    }

    /// Live finding B-F2 turned on egret's own seed: once a profile holds a
    /// compiled grant (`origin = 'generated'`), `create_policy` refuses every
    /// new policy with "compiled by the SP4a permission model". The baseline
    /// must still land there, and a recompile must neither delete it nor be
    /// refused by it.
    #[tokio::test]
    async fn a_profile_holding_a_compiled_grant_gets_and_keeps_the_user_baseline() {
        let Some(st) = test_state().await else {
            eprintln!("skipping a_profile_holding_a_compiled_grant_gets_and_keeps_the_user_baseline: DATABASE_URL not set");
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (engine, sid, profile) = bound_trino_engine(
            &st,
            serde_json::json!({ "delegationMode": "jwt", "auth": { "jwt": {} } }),
        )
        .await;
        let catalog_id: Uuid = sqlx::query_scalar(
            "INSERT INTO registry.catalogs (name, kind, config) VALUES ($1,'tpch','{}'::jsonb) \
             RETURNING id",
        )
        .bind(format!("ubcat_{}", Uuid::new_v4().simple()))
        .fetch_one(&db.pool)
        .await
        .expect("catalog");
        sqlx::query(
            "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
        )
        .bind(engine.id)
        .bind(catalog_id)
        .execute(&db.pool)
        .await
        .expect("attach");
        let perm = egret_perm::PermStore::new(db.pool.clone());
        let policy = egret_policy::PolicyStore::new(db.clone());
        let grant_id = perm
            .create_grant(&egret_perm::Grant {
                id: Uuid::nil(),
                effect: egret_perm::Effect::Allow,
                actions: vec![egret_perm::Action::Read],
                scope: egret_perm::Scope {
                    catalog_id,
                    schema: None,
                    table: None,
                    column: None,
                },
                principals: egret_perm::PrincipalSet {
                    include: vec![egret_perm::Principal {
                        kind: egret_perm::PrincipalKind::User,
                        r#ref: "alice".into(),
                    }],
                    exclude: vec![],
                },
                origin: egret_perm::GrantOrigin::Explicit,
            })
            .await
            .expect("grant");
        egret_perm_compile::recompile_service(&perm, &policy, sid)
            .await
            .expect("first compile");
        let origin: String =
            sqlx::query_scalar("SELECT origin FROM policy.authz_services WHERE id = $1")
                .bind(sid)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(
            origin, "generated",
            "the fixture must reach the guarded state"
        );
        // Control: the guard is live on this profile for a hand-written policy.
        let refused = policy
            .create_policy(sid, "hand-written", &serde_json::json!({}))
            .await;
        assert!(
            matches!(
                refused,
                Err(egret_policy::PolicyStoreError::CompilerOwned(_))
            ),
            "{refused:?}"
        );

        build_provision_req(&st, &engine)
            .await
            .expect("the apply must not be refused by the compiled-profile guard");
        assert_user_baseline(&stored_policies(sid).await, &profile);

        egret_perm_compile::recompile_service(&perm, &policy, sid)
            .await
            .expect("recompile with the baseline present");
        let rows = stored_policies(sid).await;
        assert_user_baseline(&rows, &profile);
        let grant_policy = format!("{}{}", egret_policy::COMPILER_GRANT_PREFIX, grant_id);
        assert!(
            rows.iter()
                .any(|(n, _, m)| *n == grant_policy && m == "compiler"),
            "the compiled grant must still be there: {rows:#?}"
        );
    }

    /// Existing engines get the baseline without an apply: the startup pass
    /// seeds every delegating Trino engine bound to a profile, and only those.
    #[tokio::test]
    async fn the_startup_pass_seeds_the_baseline_on_an_existing_engine_without_an_apply() {
        let Some(st) = test_state().await else {
            eprintln!("skipping the_startup_pass_seeds_the_baseline_on_an_existing_engine_without_an_apply: DATABASE_URL not set");
            return;
        };
        let (jwt, jwt_sid, jwt_profile) = bound_trino_engine(
            &st,
            serde_json::json!({ "delegationMode": "jwt", "auth": { "jwt": {} } }),
        )
        .await;
        let (open, open_sid, _) =
            bound_trino_engine(&st, serde_json::json!({ "delegationMode": "none" })).await;
        assert!(stored_policies(jwt_sid).await.is_empty(), "no apply ran");

        assert!(
            seed_engine_user_baseline(&st, &jwt)
                .await
                .expect("seed the jwt engine"),
            "a bound delegating engine is seeded"
        );
        assert_user_baseline(&stored_policies(jwt_sid).await, &jwt_profile);

        assert!(
            !seed_engine_user_baseline(&st, &open)
                .await
                .expect("an open engine is skipped, not an error"),
            "a non-delegating engine is not seeded"
        );
        assert!(stored_policies(open_sid).await.is_empty());

        let mut unbound = jwt.clone();
        unbound.authz_service_id = None;
        assert!(!seed_engine_user_baseline(&st, &unbound).await.unwrap());

        let mut starrocks = jwt.clone();
        starrocks.r#type = "starrocks".into();
        assert!(!seed_engine_user_baseline(&st, &starrocks).await.unwrap());

        // An EXTERNAL Trino engine, bound to a profile, as `register_external`
        // stores it: `spec = '{}'`, which `delegation_mode()` reads as
        // `impersonate`, so `supports_delegation()` is true. egret never
        // touches that cluster, so it must get nothing — not at startup, and
        // not through an apply either.
        let ext_profile = format!("ubext_{}", Uuid::new_v4().simple());
        let ext_svc = st
            .authz
            .create_service(&ext_profile, "trino", &serde_json::json!({}))
            .await
            .expect("create external engine's profile");
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 2)
            .await
            .expect("connect");
        let ext_id: Uuid = sqlx::query_scalar(
            "INSERT INTO engine_mgmt.engines \
               (name, type, namespace, spec, status, origin, external_host, external_port, \
                ranger_plugin_version, authz_service_id) \
             VALUES ($1, 'trino', '', '{}'::jsonb, 'running', 'external', 'trino.example', 8443, \
                     '2.8.0', $2) \
             RETURNING id",
        )
        .bind(format!("trino-ext-{}", Uuid::new_v4().simple()))
        .bind(ext_svc.id)
        .fetch_one(&db.pool)
        .await
        .expect("register external engine");
        let external = st
            .engine_mgmt
            .get_engine(ext_id)
            .await
            .unwrap()
            .expect("external engine");
        assert!(
            crate::external::is_external(&external) && external.supports_delegation(),
            "the fixture must be the case the guard exists for"
        );
        assert!(
            !seed_engine_user_baseline(&st, &external)
                .await
                .expect("an external engine is skipped, not an error"),
            "an external engine is not seeded"
        );
        assert!(stored_policies(ext_svc.id).await.is_empty());
        // The apply path cannot reach the seed for it either: every apply
        // handler refuses an external engine, and the build itself stops at
        // `stored_image` (an external spec carries no image) before any seed.
        assert!(build_provision_req(&st, &external).await.is_err());
        assert!(stored_policies(ext_svc.id).await.is_empty());
    }

    /// `resolve_ranger_service_name` must fail LOUDLY — never fall back to the
    /// engine's name — when `authz_service_id` names no `policy.authz_services`
    /// row. Bind/unbind never delete a profile row (standing constraint), and
    /// the FK (`migration 0058`, `ON DELETE SET NULL`) means an engine row
    /// itself can never end up pointing at a missing id in practice — but the
    /// helper is the last line of defense against that invariant ever being
    /// violated, and a silent fall-back to `engine.name` is exactly the defect
    /// SP4a Task 3b closes. Exercised directly (no engine row needed) since the
    /// FK makes the dangling state unreachable through the engine table.
    #[tokio::test]
    async fn resolve_ranger_service_name_fails_loudly_for_a_dangling_id() {
        let Some(st) = test_state().await else {
            eprintln!(
                "skipping resolve_ranger_service_name_fails_loudly_for_a_dangling_id: DATABASE_URL not set"
            );
            return;
        };
        let err = resolve_ranger_service_name(&st, Some(i64::MAX))
            .await
            .expect_err("a dangling authz_service_id must be a loud failure");
        assert!(matches!(err, CpError::Internal), "got {err:?}");
    }

    /// The happy path `resolve_ranger_service_name_fails_loudly_for_a_dangling_id`
    /// is the mirror of: `None` stays `None` (unbound engines carry no
    /// `RangerBinding`, unaffected by this fix).
    #[tokio::test]
    async fn resolve_ranger_service_name_is_none_when_unbound() {
        let Some(st) = test_state().await else {
            eprintln!(
                "skipping resolve_ranger_service_name_is_none_when_unbound: DATABASE_URL not set"
            );
            return;
        };
        let name = resolve_ranger_service_name(&st, None)
            .await
            .expect("unbound must not error");
        assert_eq!(name, None);
    }

    /// SP3c-2 Task 6. A StarRocks engine's builtin storage volume is derived
    /// from the warehouse of its PRIMARY ATTACHMENT; an engine with NO
    /// primary is refused rather than silently coming up with no storage.
    /// There is no `engines.warehouse_id` column any more to fall back to.
    #[tokio::test]
    async fn starrocks_storage_comes_from_the_primary_attachment() {
        let Some(st) = test_state().await else {
            eprintln!(
                "skipping starrocks_storage_comes_from_the_primary_attachment: DATABASE_URL not set"
            );
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (led_wh, led_secret) = seed_sr_warehouse(&st, &db).await;

        let engine = st
            .engine_mgmt
            .create_engine(
                &format!("sr-{}", Uuid::new_v4().simple()),
                "starrocks",
                "egret-engines",
                &serde_json::json!({ "starrocksConfig": { "feImage": "starrocks/fe-ubuntu:4.1.6", "cnImage": "starrocks/cn-ubuntu:4.1.6", "feReplicas": 1 } }),
                None,
            )
            .await
            .expect("create engine row");

        // (a) No attachment at all: refused rather than silently coming up
        //     with no storage.
        let Err(err) = build_provision_req_with(&st, &engine, BuildOpts::default()).await else {
            panic!("an engine with no primary attachment must be refused");
        };
        let msg = err.to_string();
        assert!(
            matches!(err, CpError::BadRequest(_)),
            "a refusal an operator can act on, not a 500; err={msg}"
        );
        assert!(
            msg.contains("no primary catalog with a warehouse"),
            "the refusal names what is missing; err={msg}"
        );
        assert!(
            msg.contains(&engine.name),
            "the refusal names the engine; err={msg}"
        );

        // (b) Attach `led_wh`'s catalog as the primary.
        let registry = st.engine_mgmt.registry();
        let (led_catalog, _) = registry
            .ensure_warehouse_catalog(
                &WarehouseIdentity {
                    id: led_wh.id,
                    name: &led_wh.name,
                    workspace_id: led_wh.workspace_id,
                },
                None,
            )
            .await
            .expect("warehouse catalog");
        registry
            .insert_attachment(&egret_catalog_registry::NewAttachment {
                engine_id: engine.id,
                catalog_id: led_catalog.id,
                catalog_client_id: None,
                credential_secret_id: None,
                minted_client: false,
                applied_now: true,
                is_primary: true,
                created_at: None,
                apply_status: egret_catalog_registry::store::apply_status::APPLIED,
            })
            .await
            .expect("attach primary");

        let built = build_provision_req_with(&st, &engine, BuildOpts::default())
            .await
            .expect("build with a primary attachment");
        let sr = built.req.starrocks.as_ref().expect("starrocks config");
        assert_eq!(
            sr.storage_secret_key, led_secret,
            "storage follows the primary attachment's warehouse"
        );
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

    /// Like `NoopProvisioner`, but `update` fails — a Kubernetes rejection.
    struct FailingUpdateProvisioner;
    #[async_trait::async_trait]
    impl egret_engine_mgmt_api::EngineProvisioningPort for FailingUpdateProvisioner {
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
            Err("injected update failure".into())
        }
        async fn delete(&self, _kind: &str, _namespace: &str, _name: &str) -> Result<(), String> {
            Ok(())
        }
        async fn restart(&self, _kind: &str, _namespace: &str, _name: &str) -> Result<(), String> {
            Ok(())
        }
    }

    /// Counts `update` calls, so a test can assert the provisioner was never
    /// touched (e.g. a CAS miss must bail before it).
    #[derive(Default)]
    struct CountingProvisioner {
        updates: std::sync::atomic::AtomicUsize,
    }
    #[async_trait::async_trait]
    impl egret_engine_mgmt_api::EngineProvisioningPort for CountingProvisioner {
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
            self.updates
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        async fn delete(&self, _kind: &str, _namespace: &str, _name: &str) -> Result<(), String> {
            Ok(())
        }
        async fn restart(&self, _kind: &str, _namespace: &str, _name: &str) -> Result<(), String> {
            Ok(())
        }
    }

    /// `provision` AND `delete` both fail — a cluster compensation can't reach
    /// the cluster at all (I1: the compensation's OWN cluster delete fails).
    struct FailingProvisionAndDeleteProvisioner;
    #[async_trait::async_trait]
    impl egret_engine_mgmt_api::EngineProvisioningPort for FailingProvisionAndDeleteProvisioner {
        async fn provision(&self, _req: &egret_engine_mgmt_api::EngineSpec) -> Result<(), String> {
            Err("injected provision failure".into())
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
            Err("injected delete failure".into())
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
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (_wh, _secret) = seed_sr_warehouse(&st, &db).await;
        let engine = st
            .engine_mgmt
            .create_engine(
                &format!("sr-{}", Uuid::new_v4().simple()),
                "starrocks",
                "egret-engines",
                &serde_json::json!({ "starrocksConfig": { "feImage": "starrocks/fe-ubuntu:4.1.6", "cnImage": "starrocks/cn-ubuntu:4.1.6" } }),
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
        let dsn = egret_test_db::url().await.unwrap();
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
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (_wh, _s) = seed_sr_warehouse(&st, &db).await;

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
                &serde_json::json!({ "imageVersion": "483", "trinoConfig": {} }),
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

    /// The create handler validates the INGRESS `tlsCertSecretRef` up front: an
    /// Opaque (non-TLS) secret is rejected with a 400, mirroring the coordinator
    /// `tls_cert_secret_ref` check (Task 4).
    #[tokio::test]
    async fn create_rejects_ingress_tls_cert_ref_non_tls() {
        let Some(st) = test_state().await else {
            eprintln!("skipping create_rejects_ingress_tls_cert_ref_non_tls: DATABASE_URL not set");
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (wh, _s) = seed_sr_warehouse(&st, &db).await;

        // Engines only provision against a lakekeeper-backed warehouse; mark it
        // so the handler passes the provisioned check and reaches TLS validation.
        egret_catalog_engine::CatalogStore::new(db.clone())
            .set_lakekeeper_warehouse_id(wh.id, "lk-test")
            .await
            .expect("mark warehouse provisioned");

        // A single-value create is an Opaque secret, not kubernetes.io/tls.
        let opaque = st
            .secrets
            .create(
                None,
                &format!("op-{}", Uuid::new_v4().simple()),
                None,
                "plain",
            )
            .await
            .expect("create opaque secret");

        let body = CreateEngineBody {
            name: format!("trino-{}", Uuid::new_v4().simple()),
            warehouse_id: Some(wh.id),
            catalogs: Vec::new(),
            image_version: None,
            coordinator_replicas: 1,
            worker_replicas: 1,
            cpu_min: "1".into(),
            cpu_max: "1".into(),
            memory: "1Gi".into(),
            trino_config: Some(serde_json::json!({
                "ingress": { "enabled": true, "host": "h", "tlsCertSecretRef": opaque }
            })),
            authz_service_id: None,
            kind: "trino".into(),
            starrocks: None,
            tls_cert_secret_ref: None,
        };

        let principal = egret_core::context::Principal {
            user_id: egret_core::ids::UserId::new(),
            email: "admin@egret.local".into(),
            platform_roles: vec!["platform_admin".into()],
            permissions: vec![],
            group_ids: vec![],
            impersonator: None,
        };

        match create_engine(State(st), Auth(principal), Json(body)).await {
            Err(CpError::BadRequest(_)) => {}
            Err(other) => panic!("expected BadRequest, got {other:?}"),
            Ok(_) => panic!("expected create to reject non-TLS ingress cert ref"),
        }
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
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (wh, _secret) = seed_sr_warehouse(&st, &db).await;

        let mut body = sr_update_body(None);
        body.name = format!("sr-{}", Uuid::new_v4().simple());
        let name = body.name.clone();

        let (code, _dto) = create_starrocks_engine(st.clone(), body, wh, Vec::new())
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

    /// Drive `create_starrocks_engine` with the given delegation knobs (mode +
    /// FE image) against a fresh warehouse, returning the reloaded stored
    /// `Engine` (or the create error). The caller wires `st` first (provisioner,
    /// jwks/issuer) so it controls the platform-JWKS capability the resolver sees.
    async fn create_sr_for_delegation(
        st: &CpState,
        db: &egret_metastore::Db,
        delegation_mode: Option<&str>,
        fe_image: Option<&str>,
    ) -> Result<Engine, CpError> {
        let (wh, _secret) = seed_sr_warehouse(st, db).await;
        let mut body = sr_update_body(Some(StarRocksConfigBody {
            delegation_mode: delegation_mode.map(str::to_string),
            fe_image: fe_image.map(str::to_string),
            ..Default::default()
        }));
        body.name = format!("sr-{}", Uuid::new_v4().simple());
        let name = body.name.clone();
        let _created = create_starrocks_engine(st.clone(), body, wh, Vec::new()).await?;
        Ok(st
            .engine_mgmt
            .list_engines()
            .await
            .expect("list_engines")
            .into_iter()
            .find(|e| e.name == name)
            .expect("created engine present"))
    }

    /// Default-on delegation (spec 2026-07-21): an omitted mode with a
    /// platform JWKS and a JWT-capable FE image resolves to `jwt`, and JWT
    /// forces TLS on (cleartext-password login).
    #[tokio::test]
    async fn sr_create_defaults_to_jwt_with_tls_when_capable() {
        let Some(mut st) = test_state().await else {
            eprintln!(
                "skipping sr_create_defaults_to_jwt_with_tls_when_capable: DATABASE_URL not set"
            );
            return;
        };
        st.engine_provisioner = Some(std::sync::Arc::new(NoopProvisioner));
        // test_state already sets engine_jwks_uri + engine_oidc_issuer.
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");

        let eng = create_sr_for_delegation(&st, &db, None, Some("starrocks/fe-ubuntu:4.1"))
            .await
            .expect("create");
        assert_eq!(eng.delegation_mode(), "jwt");
        assert!(eng.supports_jwt_delegation());
        assert_eq!(
            eng.spec["starrocksConfig"]["delegation"]["tlsEnabled"],
            true
        );
    }

    /// No platform JWKS ⇒ the omitted default degrades to `impersonate`.
    #[tokio::test]
    async fn sr_create_omitted_degrades_to_impersonate_without_jwks() {
        let Some(mut st) = test_state().await else {
            eprintln!("skipping sr_create_omitted_degrades_to_impersonate_without_jwks: DATABASE_URL not set");
            return;
        };
        st.engine_provisioner = Some(std::sync::Arc::new(NoopProvisioner));
        st.engine_jwks_uri.clear();
        st.engine_oidc_issuer.clear();
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");

        let eng = create_sr_for_delegation(&st, &db, None, None)
            .await
            .expect("create");
        assert_eq!(eng.delegation_mode(), "impersonate");
    }

    /// A JWT-capable platform but a pre-3.5 FE image (no `authentication_jwt`)
    /// degrades the omitted default to `impersonate`.
    #[tokio::test]
    async fn sr_create_omitted_degrades_to_impersonate_on_pre_3_5_image() {
        let Some(mut st) = test_state().await else {
            eprintln!("skipping sr_create_omitted_degrades_to_impersonate_on_pre_3_5_image: DATABASE_URL not set");
            return;
        };
        st.engine_provisioner = Some(std::sync::Arc::new(NoopProvisioner));
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");

        let eng = create_sr_for_delegation(&st, &db, None, Some("starrocks/fe-ubuntu:3.4"))
            .await
            .expect("create");
        assert_eq!(eng.delegation_mode(), "impersonate");
    }

    /// An EXPLICIT `jwt` the platform can't satisfy (no JWKS) hard-fails with a
    /// 400 that names the fix.
    #[tokio::test]
    async fn sr_create_explicit_jwt_without_jwks_400s() {
        let Some(mut st) = test_state().await else {
            eprintln!("skipping sr_create_explicit_jwt_without_jwks_400s: DATABASE_URL not set");
            return;
        };
        st.engine_provisioner = Some(std::sync::Arc::new(NoopProvisioner));
        st.engine_jwks_uri.clear();
        st.engine_oidc_issuer.clear();
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");

        let err = create_sr_for_delegation(&st, &db, Some("jwt"), None)
            .await
            .expect_err("explicit jwt without jwks must 400");
        match err {
            CpError::BadRequest(m) => {
                assert!(m.contains("public_url"), "message names the fix: {m}")
            }
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    /// The explicit `"none"` opt-out creates an open engine (no delegation block).
    #[tokio::test]
    async fn sr_create_explicit_none_is_open() {
        let Some(mut st) = test_state().await else {
            eprintln!("skipping sr_create_explicit_none_is_open: DATABASE_URL not set");
            return;
        };
        st.engine_provisioner = Some(std::sync::Arc::new(NoopProvisioner));
        // JWKS present, but "none" must still win.
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");

        let eng = create_sr_for_delegation(&st, &db, Some("none"), None)
            .await
            .expect("create");
        assert_eq!(eng.delegation_mode(), "none");
        assert!(!eng.supports_delegation());
    }

    /// §6 regression: an update that omits `delegationMode` must PRESERVE the
    /// stored mode (only create resolves the default). Create an open (`"none"`)
    /// engine, then update an unrelated field with no delegation ⇒ stays `none`,
    /// NOT flipped to `jwt`.
    #[tokio::test]
    async fn sr_update_with_omitted_mode_preserves_stored() {
        let Some(mut st) = test_state().await else {
            eprintln!(
                "skipping sr_update_with_omitted_mode_preserves_stored: DATABASE_URL not set"
            );
            return;
        };
        st.engine_provisioner = Some(std::sync::Arc::new(NoopProvisioner));
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");

        // Create an open engine (explicit "none"), JWKS present.
        let eng = create_sr_for_delegation(&st, &db, Some("none"), None)
            .await
            .expect("create");
        assert_eq!(eng.delegation_mode(), "none");

        // Update an unrelated field with omitted delegationMode. The update path
        // does NOT resolve the default; build_starrocks_spec preserves the stored
        // (null) delegation block. Provision may fail (no k8s); the spec is
        // persisted before that, which is what we assert.
        let body = sr_update_body(Some(StarRocksConfigBody {
            fe_replicas: Some(2),
            ..Default::default()
        }));
        let _ = update_starrocks_engine(st.clone(), eng.clone(), body).await;

        let reloaded = st
            .engine_mgmt
            .get_engine(eng.id)
            .await
            .expect("reload")
            .expect("present");
        assert_eq!(
            reloaded.delegation_mode(),
            "none",
            "omitted update must not flip a stored open engine to jwt"
        );
    }

    // ── Trino create-side default-on delegation (spec 2026-07-21) ───────────
    /// Drive the FULL `create_engine` handler for a TRINO engine with the given
    /// wire `trinoConfig` (`None` = a bare create with no `trinoConfig` key at
    /// all) against a fresh lakekeeper-provisioned warehouse, returning the
    /// reloaded stored `Engine` (or the create error). The caller wires `st`
    /// (provisioner, jwks/issuer) first so it controls the platform-JWKS
    /// capability the resolver sees. Mirrors `create_sr_for_delegation`, but the
    /// Trino path resolves + persists the default INSIDE `create_engine`, so it
    /// must go through the handler (not a StarRocks-style sub-fn).
    async fn create_trino_for_delegation(
        st: &CpState,
        db: &egret_metastore::Db,
        trino_config: Option<serde_json::Value>,
    ) -> Result<Engine, CpError> {
        let (wh, _secret) = seed_sr_warehouse(st, db).await;
        // Engines only provision against a lakekeeper-backed warehouse; mark it so
        // the handler passes the provisioned check and reaches the Trino path.
        egret_catalog_engine::CatalogStore::new(db.clone())
            .set_lakekeeper_warehouse_id(wh.id, "lk-test")
            .await
            .expect("mark warehouse provisioned");
        let name = format!("trino-{}", Uuid::new_v4().simple());
        let body = CreateEngineBody {
            name: name.clone(),
            warehouse_id: Some(wh.id),
            catalogs: Vec::new(),
            image_version: None,
            coordinator_replicas: 1,
            worker_replicas: 1,
            cpu_min: "1".into(),
            cpu_max: "1".into(),
            memory: "1Gi".into(),
            trino_config,
            authz_service_id: None,
            kind: "trino".into(),
            starrocks: None,
            tls_cert_secret_ref: None,
        };
        let principal = egret_core::context::Principal {
            user_id: egret_core::ids::UserId::new(),
            email: "admin@egret.local".into(),
            platform_roles: vec!["platform_admin".into()],
            permissions: vec![],
            group_ids: vec![],
            impersonator: None,
        };
        let _ = create_engine(State(st.clone()), Auth(principal), Json(body)).await?;
        Ok(st
            .engine_mgmt
            .list_engines()
            .await
            .expect("list_engines")
            .into_iter()
            .find(|e| e.name == name)
            .expect("created engine present"))
    }

    /// Default-on delegation: a `trinoConfig` present but with NO `delegationMode`,
    /// on a JWT-capable platform, resolves to `jwt` — and an `auth.jwt` block is
    /// synthesized so `resolve_auth` templates the platform JWKS + `aud:"engine"`.
    #[tokio::test]
    async fn trino_create_defaults_to_jwt_when_capable() {
        let Some(mut st) = test_state().await else {
            eprintln!("skipping trino_create_defaults_to_jwt_when_capable: DATABASE_URL not set");
            return;
        };
        st.engine_provisioner = Some(std::sync::Arc::new(NoopProvisioner));
        // test_state already sets engine_jwks_uri + engine_oidc_issuer (JWT-capable).
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");

        let eng = create_trino_for_delegation(&st, &db, Some(serde_json::json!({})))
            .await
            .expect("create");
        assert!(
            eng.is_jwt_delegation(),
            "omitted mode ⇒ jwt when JWT-capable"
        );
        assert!(
            eng.spec["trinoConfig"]["auth"]["jwt"].is_object(),
            "jwt resolution must synthesize an auth.jwt object"
        );
    }

    /// No platform JWKS ⇒ the omitted default degrades to `impersonate`.
    #[tokio::test]
    async fn trino_create_omitted_degrades_to_impersonate_without_jwks() {
        let Some(mut st) = test_state().await else {
            eprintln!("skipping trino_create_omitted_degrades_to_impersonate_without_jwks: DATABASE_URL not set");
            return;
        };
        st.engine_provisioner = Some(std::sync::Arc::new(NoopProvisioner));
        st.engine_jwks_uri.clear();
        st.engine_oidc_issuer.clear();
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");

        let eng = create_trino_for_delegation(&st, &db, Some(serde_json::json!({})))
            .await
            .expect("create");
        assert!(eng.is_impersonate_delegation());
        assert!(!eng.is_jwt_delegation());
    }

    /// The explicit `"none"` opt-out creates an open engine — neither jwt nor
    /// impersonate delegation. NOTE: Trino's `delegation_mode()` defaults an
    /// ABSENT key to `impersonate`, so `"none"` must be PERSISTED explicitly (not
    /// removed) for the engine to read back as open.
    #[tokio::test]
    async fn trino_create_explicit_none_is_open() {
        let Some(mut st) = test_state().await else {
            eprintln!("skipping trino_create_explicit_none_is_open: DATABASE_URL not set");
            return;
        };
        st.engine_provisioner = Some(std::sync::Arc::new(NoopProvisioner));
        // JWKS present, but "none" must still win.
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");

        let eng = create_trino_for_delegation(
            &st,
            &db,
            Some(serde_json::json!({ "delegationMode": "none" })),
        )
        .await
        .expect("create");
        assert!(!eng.is_jwt_delegation());
        assert!(!eng.is_impersonate_delegation());
    }

    /// Edge case: a bare Trino create (NO `trinoConfig` key at all) still applies
    /// the default — a synthesized `trinoConfig` comes up `jwt` on a JWT-capable
    /// platform.
    #[tokio::test]
    async fn trino_create_bare_config_defaults_to_jwt() {
        let Some(mut st) = test_state().await else {
            eprintln!("skipping trino_create_bare_config_defaults_to_jwt: DATABASE_URL not set");
            return;
        };
        st.engine_provisioner = Some(std::sync::Arc::new(NoopProvisioner));
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");

        let eng = create_trino_for_delegation(&st, &db, None)
            .await
            .expect("create");
        assert!(eng.is_jwt_delegation(), "bare create still defaults to jwt");
        assert!(eng.spec["trinoConfig"]["auth"]["jwt"].is_object());
    }

    /// An EXPLICIT `jwt` the platform can't satisfy (no JWKS) hard-fails on create
    /// with a 400 that names the fix — surfaced by the create-path
    /// `resolve_delegation` call (before provisioning).
    #[tokio::test]
    async fn trino_create_explicit_jwt_without_jwks_400s() {
        let Some(mut st) = test_state().await else {
            eprintln!("skipping trino_create_explicit_jwt_without_jwks_400s: DATABASE_URL not set");
            return;
        };
        st.engine_provisioner = Some(std::sync::Arc::new(NoopProvisioner));
        st.engine_jwks_uri.clear();
        st.engine_oidc_issuer.clear();
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");

        let err = create_trino_for_delegation(
            &st,
            &db,
            Some(serde_json::json!({ "delegationMode": "jwt" })),
        )
        .await
        .expect_err("explicit jwt without jwks must 400");
        match err {
            CpError::BadRequest(m) => {
                assert!(m.contains("public_url"), "message names the fix: {m}")
            }
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    /// Orphan-client guard (holistic review, Finding A): the explicit-jwt-without-
    /// JWKS 400 must fire BEFORE the auto-wired catalog client is minted, so a
    /// rejected create leaves NO `engine-{name}` client row behind. This proves the
    /// resolve-delegation → create_client ordering (mirrors StarRocks). Before the
    /// fix, `create_client` ran first and the resolver's `?` early-return leaked the
    /// client every time.
    #[tokio::test]
    async fn trino_create_explicit_jwt_without_jwks_leaves_no_orphan_client() {
        let Some(mut st) = test_state().await else {
            eprintln!(
                "skipping trino_create_explicit_jwt_without_jwks_leaves_no_orphan_client: DATABASE_URL not set"
            );
            return;
        };
        st.engine_provisioner = Some(std::sync::Arc::new(NoopProvisioner));
        st.engine_jwks_uri.clear();
        st.engine_oidc_issuer.clear();
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");

        // Seed a lakekeeper-backed warehouse (same setup create_trino_for_delegation
        // uses) so the handler reaches the Trino delegation-resolution block.
        let (wh, _secret) = seed_sr_warehouse(&st, &db).await;
        egret_catalog_engine::CatalogStore::new(db.clone())
            .set_lakekeeper_warehouse_id(wh.id, "lk-test")
            .await
            .expect("mark warehouse provisioned");

        let name = format!("trino-{}", Uuid::new_v4().simple());
        let body = CreateEngineBody {
            name: name.clone(),
            warehouse_id: Some(wh.id),
            catalogs: Vec::new(),
            image_version: None,
            coordinator_replicas: 1,
            worker_replicas: 1,
            cpu_min: "1".into(),
            cpu_max: "1".into(),
            memory: "1Gi".into(),
            trino_config: Some(serde_json::json!({ "delegationMode": "jwt" })),
            authz_service_id: None,
            kind: "trino".into(),
            starrocks: None,
            tls_cert_secret_ref: None,
        };
        let principal = egret_core::context::Principal {
            user_id: egret_core::ids::UserId::new(),
            email: "admin@egret.local".into(),
            platform_roles: vec!["platform_admin".into()],
            permissions: vec![],
            group_ids: vec![],
            impersonator: None,
        };

        // `create_engine`'s Ok type (`Json<EngineDto>`) isn't `Debug`, so match
        // rather than `.expect_err`.
        let err = match create_engine(State(st.clone()), Auth(principal), Json(body)).await {
            Ok(_) => panic!("explicit jwt without jwks must 400, but create succeeded"),
            Err(e) => e,
        };
        assert!(
            matches!(err, CpError::BadRequest(_)),
            "expected 400, got {err:?}"
        );

        // No `engine-{name}` catalog client may exist — the 400 fired before the
        // client was minted, so nothing was orphaned.
        let clients = st.catalog.list_clients(wh.id).await.expect("list clients");
        assert!(
            !clients.iter().any(|c| c.name == format!("engine-{name}")),
            "jwt-no-JWKS 400 must not orphan a catalog client; found: {:?}",
            clients.iter().map(|c| &c.name).collect::<Vec<_>>()
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
        let dsn = egret_test_db::url().await.unwrap();
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
                &serde_json::json!({ "starrocksConfig": { "feImage": "starrocks/fe-ubuntu:4.1.6", "cnImage": "starrocks/cn-ubuntu:4.1.6" } }),
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

    /// D6: `fe_target` is the ONE place that decides how egret dials a
    /// StarRocks FE — true since Task 5, which moved the bootstrap's own
    /// statements (the root-login probe included) onto it and deleted the
    /// `run_starrocks_ddl` that passed `tls_cert_pem = None` unconditionally,
    /// so that an OAuth2 credential and the FE root password crossed the wire
    /// in cleartext on a TLS-capable engine.
    ///
    /// What it decides, and this test pins: the certificate is threaded
    /// through exactly when [`sr_tls_enabled`] says the FE serves TLS —
    /// delegation configured, `tlsEnabled` not turned off, DEFAULTING ON —
    /// whatever `coordinator_cert` holds; and the root password only when the
    /// engine carries a `root_secret_id`.
    #[tokio::test]
    async fn fe_target_resolves_root_password_and_gates_the_cert_on_delegation_tls() {
        let Some(st) = test_state().await else {
            eprintln!(
                "skipping fe_target_resolves_root_password_and_gates_the_cert_on_delegation_tls: DATABASE_URL not set"
            );
            return;
        };
        let legacy = seed_sr_engine(&st).await;
        let (secret_id, plaintext) = ensure_root_password_secret(&st, &legacy)
            .await
            .expect("mint root password");
        let base = Engine {
            root_secret_id: Some(secret_id),
            coordinator_cert: Some("CERTPEM".into()),
            ..legacy.clone()
        };

        // tlsEnabled=true: the pinned cert is threaded through.
        let mut with_tls = base.clone();
        with_tls.spec = serde_json::json!({
            "starrocksConfig": { "delegation": { "mode": "impersonate", "tlsEnabled": true } }
        });
        let fe = crate::starrocks_ddl::fe_target(&st, &with_tls)
            .await
            .expect("fe_target (tls on)");
        assert_eq!(fe.root_password.as_deref(), Some(plaintext.as_str()));
        assert_eq!(fe.tls_cert_pem.as_deref(), Some("CERTPEM"));
        assert_eq!(fe.host, with_tls.coordinator_host());
        assert_eq!(fe.port, with_tls.coordinator_port() as u16);

        // tlsEnabled OMITTED on a delegating engine: the cert is threaded
        // through, because that is the engine
        // `build_starrocks_provision_req` provisioned — `sr_tls_enabled`
        // defaults it ON, and before Task 5 this function defaulted it OFF, so
        // the commonest delegating engine was given a TLS listener and then
        // dialled in cleartext (D6).
        let mut default_tls = base.clone();
        default_tls.spec = serde_json::json!({
            "starrocksConfig": { "delegation": { "mode": "jwt" } }
        });
        let fe = crate::starrocks_ddl::fe_target(&st, &default_tls)
            .await
            .expect("fe_target (tls defaulted)");
        assert_eq!(
            fe.tls_cert_pem.as_deref(),
            Some("CERTPEM"),
            "an omitted tlsEnabled is TLS-ON, exactly as the provisioner reads it"
        );

        // tlsEnabled=false: the cert is withheld even though the engine has
        // one pinned — the FE isn't actually serving TLS.
        let mut without_tls = base.clone();
        without_tls.spec = serde_json::json!({
            "starrocksConfig": { "delegation": { "mode": "impersonate", "tlsEnabled": false } }
        });
        let fe = crate::starrocks_ddl::fe_target(&st, &without_tls)
            .await
            .expect("fe_target (tls off)");
        assert!(fe.tls_cert_pem.is_none());

        // No delegation block at all: same as tlsEnabled=false.
        let mut no_delegation = base.clone();
        no_delegation.spec = serde_json::json!({ "starrocksConfig": {} });
        let fe = crate::starrocks_ddl::fe_target(&st, &no_delegation)
            .await
            .expect("fe_target (no delegation)");
        assert!(fe.tls_cert_pem.is_none());

        // Legacy engine (no root_secret_id): stays passwordless.
        let fe = crate::starrocks_ddl::fe_target(&st, &legacy)
            .await
            .expect("fe_target (legacy)");
        assert!(fe.root_password.is_none());
    }

    /// A minimal StarRocks-shaped `CreateEngineBody` for update tests (only
    /// `kind` + `starrocks` are read by `update_starrocks_engine`; the Trino
    /// wizard fields are ignored).
    fn sr_update_body(starrocks: Option<StarRocksConfigBody>) -> CreateEngineBody {
        CreateEngineBody {
            name: "ignored".into(),
            warehouse_id: Some(Uuid::nil()),
            catalogs: Vec::new(),
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

    /// Seed a stored StarRocks engine ROW (1 FE / 1 CN, no delegation) for the
    /// update-path tests. The row alone: tests that reach a path deriving the
    /// engine's warehouse (anything building a provisioning request) must also
    /// call [`attach_primary_warehouse_catalog`], because the primary
    /// attachment is where that warehouse comes from — there is no
    /// `engines.warehouse_id` column any more. Tests that arrange their own attachments
    /// (`the_bootstrap_converges_every_attachment_in_name_order`) need the
    /// bare row.
    async fn seed_sr_engine(st: &CpState) -> Engine {
        let spec = serde_json::json!({
            "imageVersion": DEFAULT_ENGINE_IMAGES.starrocks_fe_image,
            "starrocksConfig": {
                "feImage": DEFAULT_ENGINE_IMAGES.starrocks_fe_image,
                "cnImage": DEFAULT_ENGINE_IMAGES.starrocks_cn_image,
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
                &spec,
                None,
            )
            .await
            .expect("create engine row")
    }

    /// Attach a warehouse's registry catalog to an engine as its PRIMARY, the
    /// way both create paths do. Test fixtures that write an engine row
    /// directly need it: since SP3c-2 Task 5 the primary attachment is where
    /// the engine's warehouse comes from.
    async fn attach_primary_warehouse_catalog(st: &CpState, engine: &Engine, wh_id: Uuid) {
        let wh = st
            .catalog
            .get_warehouse(wh_id)
            .await
            .expect("get_warehouse")
            .expect("warehouse present");
        let registry = st.engine_mgmt.registry();
        let (catalog, _) = registry
            .ensure_warehouse_catalog(
                &WarehouseIdentity {
                    id: wh.id,
                    name: &wh.name,
                    workspace_id: wh.workspace_id,
                },
                None,
            )
            .await
            .expect("warehouse catalog");
        registry
            .insert_attachment(&egret_catalog_registry::NewAttachment {
                engine_id: engine.id,
                catalog_id: catalog.id,
                catalog_client_id: None,
                credential_secret_id: None,
                minted_client: false,
                applied_now: true,
                is_primary: true,
                created_at: None,
                apply_status: egret_catalog_registry::store::apply_status::APPLIED,
            })
            .await
            .expect("attach primary");
    }

    #[test]
    fn bootstrap_stops_when_the_engine_is_gone_renamed_deleting_or_failed() {
        let mut e = dummy_engine();
        e.name = "sr1".into();
        e.status = crate::engine_status::RUNNING.into();
        assert!(bootstrap_target_is_current(Some(&e), "sr1"));
        assert!(
            !bootstrap_target_is_current(None, "sr1"),
            "deleted (or re-created under a new id)"
        );
        assert!(
            !bootstrap_target_is_current(Some(&e), "sr2"),
            "not the engine it was spawned for"
        );
        e.status = crate::engine_status::DELETING.into();
        assert!(
            !bootstrap_target_is_current(Some(&e), "sr1"),
            "delete in progress"
        );
        e.status = crate::engine_status::FAILED.into();
        assert!(
            !bootstrap_target_is_current(Some(&e), "sr1"),
            "Task 12's compensation may not have removed it; cluster objects may already be gone"
        );
    }

    #[tokio::test]
    async fn engine_still_current_reads_the_row() {
        let Some(st) = test_state().await else {
            eprintln!("skipping engine_still_current_reads_the_row: DATABASE_URL not set");
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (_wh, _secret) = seed_sr_warehouse(&st, &db).await;
        let engine = seed_sr_engine(&st).await;
        let mut errors = 0u32;
        assert!(engine_still_current(&st.engine_mgmt, engine.id, &engine.name, &mut errors).await);
        assert_eq!(
            errors, 0,
            "a successful read must not touch the error counter"
        );
        st.engine_mgmt.delete_engine(engine.id).await.unwrap();
        assert!(!engine_still_current(&st.engine_mgmt, engine.id, &engine.name, &mut errors).await);
    }

    /// A transient DB read failure must not permanently abandon a healthy
    /// bootstrap (F3): fewer than [`MAX_CONSECUTIVE_ENGINE_READ_ERRORS`]
    /// consecutive `get_engine` errors are tolerated (treated as "still
    /// current"), the threshold-th consecutive one stops it, and a
    /// subsequent successful read resets the counter — proven against a real
    /// `EngineMgmtStore` whose pool has been closed, so every read
    /// deterministically errors.
    #[tokio::test]
    async fn engine_still_current_tolerates_a_few_consecutive_read_errors_then_stops() {
        let Some(st) = test_state().await else {
            eprintln!(
                "skipping engine_still_current_tolerates_a_few_consecutive_read_errors_then_stops: DATABASE_URL not set"
            );
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (_wh, _secret) = seed_sr_warehouse(&st, &db).await;
        let engine = seed_sr_engine(&st).await;

        // A store whose pool is already closed: every `get_engine` call
        // errors deterministically, standing in for a transient DB blip
        // without any real network flakiness in the test.
        let broken_db = egret_metastore::Db::connect(&dsn, 1)
            .await
            .expect("connect (before closing)");
        broken_db.pool.close().await;
        let broken_store = crate::EngineMgmtStore::new(broken_db);

        let mut consecutive = 0u32;
        for i in 0..MAX_CONSECUTIVE_ENGINE_READ_ERRORS - 1 {
            assert!(
                engine_still_current(&broken_store, engine.id, &engine.name, &mut consecutive)
                    .await,
                "read error #{i} (below the threshold) must be tolerated"
            );
        }
        assert_eq!(consecutive, MAX_CONSECUTIVE_ENGINE_READ_ERRORS - 1);

        // The threshold-th CONSECUTIVE failure gives up.
        assert!(
            !engine_still_current(&broken_store, engine.id, &engine.name, &mut consecutive).await,
            "the {MAX_CONSECUTIVE_ENGINE_READ_ERRORS}th consecutive read error must stop the bootstrap"
        );

        // A later successful read against the real store resets the
        // counter — failures only count when back-to-back.
        let mut consecutive2 = MAX_CONSECUTIVE_ENGINE_READ_ERRORS - 1;
        assert!(
            engine_still_current(&st.engine_mgmt, engine.id, &engine.name, &mut consecutive2).await,
            "the still-current engine must read back true"
        );
        assert_eq!(
            consecutive2, 0,
            "a successful read must reset the consecutive-error counter"
        );
    }

    /// Counts `.status()` calls — proves a checkpoint trips BEFORE the
    /// provisioner is ever touched, rather than merely reasoning about it —
    /// and reports whatever status a test sets. The DEFAULT status has no pods
    /// at all, so an FE is never ready unless a test says so: that is what the
    /// "an unreachable FE writes nothing" case needs, and it is also why
    /// nothing in this module accidentally reaches the FE.
    #[derive(Default)]
    struct CountingStatusProvisioner {
        status_calls: std::sync::atomic::AtomicUsize,
        status_response: std::sync::Mutex<Option<egret_engine_mgmt_api::EngineStatus>>,
        #[allow(clippy::type_complexity)]
        on_status: std::sync::Mutex<
            Option<Arc<dyn Fn() -> futures_util::future::BoxFuture<'static, ()> + Send + Sync>>,
        >,
    }
    impl CountingStatusProvisioner {
        fn set_status(&self, status: egret_engine_mgmt_api::EngineStatus) {
            *self.status_response.lock().unwrap() = Some(status);
        }
        fn calls(&self) -> usize {
            self.status_calls.load(std::sync::atomic::Ordering::SeqCst)
        }
        /// Run this on every `status` poll, before it answers. The cluster
        /// poll is the ONLY thing that happens between the FE-ready
        /// checkpoint and the root-login one, so it is the only place a test
        /// can make the engine stop being current in that window (fix round
        /// 1, F1).
        fn on_status(
            &self,
            hook: impl Fn() -> futures_util::future::BoxFuture<'static, ()> + Send + Sync + 'static,
        ) {
            *self.on_status.lock().unwrap() = Some(Arc::new(hook));
        }
    }
    #[async_trait::async_trait]
    impl egret_engine_mgmt_api::EngineProvisioningPort for CountingStatusProvisioner {
        async fn provision(&self, _req: &egret_engine_mgmt_api::EngineSpec) -> Result<(), String> {
            Ok(())
        }
        async fn status(
            &self,
            _kind: &str,
            _namespace: &str,
            _name: &str,
        ) -> Result<egret_engine_mgmt_api::EngineStatus, String> {
            self.status_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let hook = self.on_status.lock().unwrap().clone();
            if let Some(hook) = hook {
                hook().await;
            }
            if let Some(s) = self.status_response.lock().unwrap().clone() {
                return Ok(s);
            }
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

    /// F4 (Task 14 round-1 review): prove — by actually running
    /// `spawn_starrocks_catalog_wire`, not just its helpers — that the FIRST
    /// checkpoint stops the bootstrap before it ever calls the provisioner.
    /// Deleting that checkpoint would make `.status()` get called at least
    /// once; that regression would NOT be caught by the two helper-level
    /// unit tests above it, only by driving the real spawned task.
    ///
    /// The other five checkpoints are driven by the tests below it, through
    /// the fake FE (D8).
    #[tokio::test]
    async fn bootstrap_stops_before_any_provisioner_call_when_engine_already_gone() {
        let Some((st, fake, prov)) = test_state_with_fake_fe().await else {
            eprintln!(
                "skipping bootstrap_stops_before_any_provisioner_call_when_engine_already_gone: DATABASE_URL not set"
            );
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (_wh, _secret) = seed_sr_warehouse(&st, &db).await;
        let engine = seed_sr_engine(&st).await;
        prov.set_status(ready_fe_status(&engine.name));
        st.engine_mgmt.delete_engine(engine.id).await.unwrap();

        spawn_starrocks_catalog_wire(
            st.clone(),
            engine.id,
            engine.name.clone(),
            engine.namespace.clone(),
            None,
        )
        .await
        .expect("bootstrap task did not panic");

        assert_eq!(
            prov.calls(),
            0,
            "the engine was already deleted before spawn; the first checkpoint must stop the \
             task before it ever calls the provisioner"
        );
        assert!(
            fake.executed().is_empty(),
            "and before it says anything to the FE: {:?}",
            fake.executed()
        );
    }

    /// A `CpState` whose `starrocks_ddl` is a fake FE and whose provisioner is
    /// a controllable one, returned alongside both handles so a test can say
    /// what the cluster reports and read back what the FE was told. Everything
    /// else is [`test_state`].
    ///
    /// (Deviation from the brief, which returns only `(CpState, fake)`: the
    /// provisioner handle has to come back too, because a test that drives the
    /// FE-ready poll has to be able to CHANGE what `status` reports, and
    /// `CpState.engine_provisioner` is an `Arc<dyn EngineProvisioningPort>`
    /// with no way back to the concrete fake.)
    async fn test_state_with_fake_fe() -> Option<(
        CpState,
        crate::starrocks_ddl::FakeStarRocksDdl,
        Arc<CountingStatusProvisioner>,
    )> {
        let mut st = test_state().await?;
        let fake = crate::starrocks_ddl::FakeStarRocksDdl::default();
        let prov = Arc::new(CountingStatusProvisioner::default());
        st.starrocks_ddl = Arc::new(fake.clone());
        st.engine_provisioner = Some(prov.clone());
        Some((st, fake, prov))
    }

    /// The status a ready StarRocks FE reports: one pod whose name contains
    /// `-fe-`, which is what the bootstrap's poll actually looks for. The
    /// all-ready default of the route tests' `FakeEnginePort` will NOT do —
    /// its single pod is called `coordinator-0`.
    fn ready_fe_status(engine_name: &str) -> egret_engine_mgmt_api::EngineStatus {
        egret_engine_mgmt_api::EngineStatus {
            phase: "running".into(),
            message: None,
            pods: vec![egret_engine_mgmt_api::PodStatus {
                name: format!("{engine_name}-fe-0"),
                phase: "Running".into(),
                ready: true,
            }],
        }
    }

    /// `names.len()` hive catalogs attached to this engine as `pending`,
    /// attached IN THE ORDER GIVEN — which a caller deliberately makes
    /// different from name order, so that a test asserting name order is
    /// asserting the re-sort and not the store's own ordering (primary first,
    /// then attachment time). Returns name → catalog id.
    ///
    /// (Deviation from the brief, which says `kind: tpch`: Task 3 established
    /// that tpch/tpcds are REFUSED on StarRocks — `render_starrocks` returns
    /// `NotReadable` for them — so every attachment would fail to render and
    /// the loop under test would only ever be seen failing. `hive` renders
    /// against the warehouse's own storage connection.)
    async fn seed_pending_attachments(
        st: &CpState,
        engine: &Engine,
        wh_id: Uuid,
        names: &[&str],
    ) -> std::collections::BTreeMap<String, Uuid> {
        let wh = st
            .catalog
            .get_warehouse(wh_id)
            .await
            .expect("get_warehouse")
            .expect("warehouse present");
        let registry = st.engine_mgmt.registry();
        let mut out = std::collections::BTreeMap::new();
        for name in names {
            let def = egret_catalog_registry::CatalogDef::parse(
                "hive",
                &serde_json::json!({
                    "metastore": { "type": "hive", "uri": "thrift://hms:9083" },
                    "storage_connection_id": wh.storage_conn_id,
                }),
                &serde_json::Value::Null,
            )
            .expect("hive catalog definition");
            let row = registry
                .create_catalog(name, &def, None)
                .await
                .expect("create catalog");
            attach_pending(st, engine, row.id, None).await;
            out.insert((*name).to_string(), row.id);
        }
        out
    }

    /// The engine's PRIMARY attachment — the warehouse's own `egret-warehouse`
    /// catalog, with the OAuth2 credential its render needs — written
    /// `pending`, exactly as `create_starrocks_engine` now writes it (D1).
    async fn seed_primary_attachment(
        st: &CpState,
        engine: &Engine,
        wh_id: Uuid,
        name: &str,
    ) -> Uuid {
        let wh = st
            .catalog
            .get_warehouse(wh_id)
            .await
            .expect("get_warehouse")
            .expect("warehouse present");
        let (row, _) = st
            .engine_mgmt
            .registry()
            .ensure_warehouse_catalog(
                &WarehouseIdentity {
                    id: wh.id,
                    name: &wh.name,
                    workspace_id: wh.workspace_id,
                },
                Some(name),
            )
            .await
            .expect("ensure warehouse catalog");
        let credential = st
            .secrets
            .create(
                None,
                &format!("cred-{}", Uuid::new_v4().simple()),
                None,
                "client-id:client-secret",
            )
            .await
            .expect("create credential secret");
        attach_pending(st, engine, row.id, Some(credential)).await;
        row.id
    }

    async fn attach_pending(
        st: &CpState,
        engine: &Engine,
        catalog_id: Uuid,
        credential_secret_id: Option<Uuid>,
    ) {
        st.engine_mgmt
            .registry()
            .insert_attachment(&egret_catalog_registry::NewAttachment {
                engine_id: engine.id,
                catalog_id,
                catalog_client_id: None,
                credential_secret_id,
                minted_client: false,
                applied_now: false,
                is_primary: false,
                created_at: None,
                apply_status: egret_catalog_registry::store::apply_status::PENDING,
            })
            .await
            .expect("attach");
    }

    async fn attachment_status(
        st: &CpState,
        engine_id: Uuid,
        catalog_id: Uuid,
    ) -> egret_catalog_registry::AttachmentRow {
        st.engine_mgmt
            .registry()
            .get_attachment(engine_id, catalog_id)
            .await
            .expect("get_attachment")
            .expect("attachment present")
    }

    /// D2: the bootstrap converges EVERY attachment, in catalog-NAME order,
    /// and records each one — not just the primary, and not just in the log.
    ///
    /// The three are attached in an order that is neither name order nor
    /// primary-first (`z`, then the primary `m`, then `a`), so an
    /// implementation that simply took `list_engine_attachments`' own order
    /// would produce `m, z, a` and fail here. It also proves the settle path
    /// `create_starrocks_engine` now depends on (D1): the primary starts
    /// `pending` and ends `applied`.
    #[tokio::test]
    async fn the_bootstrap_converges_every_attachment_in_name_order() {
        let Some((st, fake, prov)) = test_state_with_fake_fe().await else {
            eprintln!("skipping the_bootstrap_converges_every_attachment_in_name_order: DATABASE_URL not set");
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (wh, _s) = seed_sr_warehouse(&st, &db).await;
        let engine = seed_sr_engine(&st).await;
        prov.set_status(ready_fe_status(&engine.name));

        let sfx = Uuid::new_v4().simple().to_string();
        let (a_name, m_name, z_name) = (format!("a_{sfx}"), format!("m_{sfx}"), format!("z_{sfx}"));
        let z = seed_pending_attachments(&st, &engine, wh.id, &[z_name.as_str()]).await[&z_name];
        let m = seed_primary_attachment(&st, &engine, wh.id, &m_name).await;
        let a = seed_pending_attachments(&st, &engine, wh.id, &[a_name.as_str()]).await[&a_name];

        spawn_starrocks_catalog_wire(
            st.clone(),
            engine.id,
            engine.name.clone(),
            engine.namespace.clone(),
            None,
        )
        .await
        .expect("bootstrap task did not panic");

        assert_eq!(
            fake.created(),
            vec![a_name.clone(), m_name.clone(), z_name.clone()],
            "name order, not attachment order; sent={:?}",
            fake.executed()
        );
        for (id, name) in [(a, &a_name), (m, &m_name), (z, &z_name)] {
            let row = attachment_status(&st, engine.id, id).await;
            assert_eq!(row.apply_status, "applied", "catalog {name}");
            assert!(row.applied_at.is_some(), "catalog {name}");
            assert!(row.apply_error.is_none(), "catalog {name}");
        }
    }

    /// Collects what a `tracing` subscriber writes, so a test can assert on a
    /// log line that is the only sink a code path has. Same shape as
    /// `tests/error_mapping.rs`'s, which is the other place in this crate that
    /// needs one.
    #[derive(Clone, Default)]
    struct Capture(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl Capture {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).expect("utf-8 log")
        }
    }

    impl std::io::Write for Capture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
        type Writer = Capture;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// A root password no shape-based redaction pass can see: eight
    /// characters, no whitespace, no quoting, no `scheme://`. Too short for
    /// `redact_opaque_runs` (>= 20), invisible to `redact_quoted` and
    /// `redact_urls`, and it leaves a well-formed message's quoting intact so
    /// the fail-closed check does not withhold the excerpt either.
    ///
    /// This is the residual `starrocks_ddl`'s module doc names as the one
    /// shape its redaction cannot close, and it is what the F2 tests are for:
    /// a REAL minted root password is 32 unbroken base64url characters, which
    /// `redact_opaque_runs` masks whatever the scrub map holds, so a test
    /// using one would pass on luck rather than on the code under test.
    const SHORT_ROOT_PW: &str = "sh0rt-pw";

    /// Replace `engine`'s FE root password with [`SHORT_ROOT_PW`] and return
    /// the reloaded row. Shared by the three F2 tests (apply, detach and the
    /// compensating drop), which differ only in which statement they make the
    /// FE refuse.
    async fn give_a_short_root_password(st: &CpState, engine: &Engine) -> Engine {
        let sid = st
            .secrets
            .create(
                None,
                &format!("egret-root-{}", engine.id),
                Some("StarRocks FE root password (short, for this test)"),
                SHORT_ROOT_PW,
            )
            .await
            .expect("store the short root password");
        st.engine_mgmt
            .set_root_secret(engine.id, sid)
            .await
            .expect("set root secret");
        st.engine_mgmt
            .get_engine(engine.id)
            .await
            .expect("reload")
            .expect("engine present")
    }

    /// An `Access denied` the way a real FE words one, echoing the credential
    /// it was given — unquoted and short, so only the scrub map can remove it.
    fn access_denied_echoing_the_root_password() -> String {
        format!("ERROR 1045 (28000): Access denied for user root using password {SHORT_ROOT_PW}")
    }

    /// F2 (final whole-branch review). Every statement the apply path sends
    /// travels on a connection that logs in as `root` with the FE root
    /// password, so that password is a secret each of those FE errors could
    /// quote — and `converge_attachment` scrubbed against the CATALOG's
    /// secrets alone, never `FeTarget::secrets()`.
    ///
    /// **The luck this test removes.** `ensure_root_password_secret` mints 24
    /// random bytes as URL-safe base64: 32 unbroken characters, which
    /// `redact_opaque_runs` masks by SHAPE whatever the scrub map holds. So
    /// the hole was closed by the length of the generated password and not by
    /// the code under test. A SHORT, unquoted password in an otherwise
    /// well-formed message is precisely the residual `starrocks_ddl`'s module
    /// doc names as the one shape its redaction cannot see — for that value,
    /// the scrub map is the ONLY thing between the FE's reply and
    /// `apply_error`, which is a column an API caller reads.
    ///
    /// Written against the bootstrap because that is the cheapest driver of
    /// `converge_attachment`; the merge it pins is in `converge_attachment`
    /// itself, shared by the attach, re-apply and Retry routes.
    #[tokio::test]
    async fn the_apply_path_scrubs_the_fe_root_password_out_of_apply_error() {
        let Some((st, fake, prov)) = test_state_with_fake_fe().await else {
            eprintln!("skipping the_apply_path_scrubs_the_fe_root_password_out_of_apply_error: DATABASE_URL not set");
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (wh, _s) = seed_sr_warehouse(&st, &db).await;
        let engine = seed_sr_engine(&st).await;

        let engine = give_a_short_root_password(&st, &engine).await;
        prov.set_status(ready_fe_status(&engine.name));

        let name = format!("a_{}", Uuid::new_v4().simple());
        let ids = seed_pending_attachments(&st, &engine, wh.id, &[name.as_str()]).await;

        // The FE refuses the convergence's very first read and echoes the
        // credential it was given, the way a real `Access denied` does. Not
        // `SELECT 1`: the root-login probe has to pass, or the bootstrap gives
        // up before it reaches an attachment at all.
        fake.fail_matching("SHOW CATALOGS", &access_denied_echoing_the_root_password());

        spawn_starrocks_catalog_wire(
            st.clone(),
            engine.id,
            engine.name.clone(),
            engine.namespace.clone(),
            None,
        )
        .await
        .expect("bootstrap task did not panic");

        let row = attachment_status(&st, engine.id, ids[&name]).await;
        assert_eq!(row.apply_status, "failed", "premise: the FE refused");
        let err = row.apply_error.expect("a failed apply records why");
        assert!(
            !err.contains(SHORT_ROOT_PW),
            "the FE root password reached `apply_error`: {err}"
        );
        // …and it was scrubbed, not withheld: a blank excerpt would pass the
        // assertion above for a reason that has nothing to do with the fix.
        assert!(
            err.contains("Access denied"),
            "premise: this message survives redaction intact apart from the \
             credential, so the scrub map is what removed it; apply_error={err}"
        );
    }

    /// The second of the three scrub sites F2 named, and the first of the two
    /// that used to pass `BTreeMap::new()` under a comment reasoning only
    /// about the statement. A `DROP CATALOG IF EXISTS` carries no secret of
    /// its own — but it still logs in as `root`, so a refused login echoes the
    /// credential, and this error goes straight into the 4xx/502 body a
    /// detach's caller reads.
    ///
    /// Driven through `apply_catalog_change` rather than the bootstrap,
    /// because that is the only way into the detach arm.
    #[tokio::test]
    async fn a_failed_detach_scrubs_the_fe_root_password_too() {
        let Some((st, fake, _prov)) = test_state_with_fake_fe().await else {
            eprintln!(
                "skipping a_failed_detach_scrubs_the_fe_root_password_too: DATABASE_URL not set"
            );
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (wh, _s) = seed_sr_warehouse(&st, &db).await;
        let engine = seed_sr_engine(&st).await;
        let engine = give_a_short_root_password(&st, &engine).await;

        let name = format!("a_{}", Uuid::new_v4().simple());
        let ids = seed_pending_attachments(&st, &engine, wh.id, &[name.as_str()]).await;

        fake.fail_matching("DROP CATALOG", &access_denied_echoing_the_root_password());

        let err = crate::catalog_apply::apply_catalog_change(
            &st,
            &engine,
            crate::catalog_apply::CatalogChange::Detach(ids[&name]),
        )
        .await
        .expect_err("premise: the FE refused the DROP");
        let msg = err.message();
        assert!(
            !msg.contains(SHORT_ROOT_PW),
            "the FE root password reached a detach's error body: {msg}"
        );
        assert!(
            msg.contains("Access denied"),
            "premise: the message survives redaction apart from the credential, \
             so the scrub map is what removed it; message={msg}"
        );
    }

    /// The third scrub site, and the other former `BTreeMap::new()`:
    /// `compensate_created_catalog`. Its only sink is a `tracing::error!`, so
    /// this captures the log rather than a return value.
    ///
    /// Reaching it takes a double fault, which is what D7 is about: the
    /// convergence CREATES the catalog, the attachment row vanishes before it
    /// can be settled (a concurrent detach — simulated here by deleting the
    /// row from the fake's create hook, so `set_apply_applied` answers
    /// `NotFound`), and then the compensating DROP fails too. That last
    /// failure is the FE error under test.
    #[tokio::test]
    async fn a_failed_compensating_drop_scrubs_the_fe_root_password_out_of_its_log() {
        let Some((st, fake, prov)) = test_state_with_fake_fe().await else {
            eprintln!("skipping a_failed_compensating_drop_scrubs_the_fe_root_password_out_of_its_log: DATABASE_URL not set");
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (wh, _s) = seed_sr_warehouse(&st, &db).await;
        let engine = seed_sr_engine(&st).await;
        let engine = give_a_short_root_password(&st, &engine).await;
        prov.set_status(ready_fe_status(&engine.name));

        let name = format!("a_{}", Uuid::new_v4().simple());
        let ids = seed_pending_attachments(&st, &engine, wh.id, &[name.as_str()]).await;
        let catalog_id = ids[&name];

        // Fault one: the row is gone by the time the CREATE is recorded.
        let registry = st.engine_mgmt.registry();
        let engine_id = engine.id;
        fake.on_create(move |_name| {
            let registry = registry.clone();
            Box::pin(async move {
                registry
                    .delete_attachment(engine_id, catalog_id)
                    .await
                    .expect("simulate a concurrent detach");
            })
        });
        // Fault two: the compensating DROP is refused, echoing the credential.
        fake.fail_matching("DROP CATALOG", &access_denied_echoing_the_root_password());

        let logs = Capture::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(logs.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .finish();
        // `set_default`, not `with_default`: the work below is async, and
        // `#[tokio::test]`'s current-thread runtime keeps it on this thread,
        // where the guard applies.
        let guard = tracing::subscriber::set_default(subscriber);
        // `tracing` caches per-callsite interest for the whole PROCESS, so a
        // sibling test in this binary that reached the line below first — with
        // no subscriber installed — can have cached it as "never" and make
        // this capture silently empty. That has bitten this repo before
        // (`tests/error_mapping.rs`'s module doc names the case). Recomputing
        // the cache under this subscriber is the documented remedy; the
        // "premise" assertion afterwards is what would catch it if it were
        // not enough, so this test can only go flaky-RED, never false-green.
        tracing::callsite::rebuild_interest_cache();

        spawn_starrocks_catalog_wire(
            st.clone(),
            engine.id,
            engine.name.clone(),
            engine.namespace.clone(),
            None,
        )
        .await
        .expect("bootstrap task did not panic");
        drop(guard);

        let text = logs.text();
        assert!(
            text.contains("the compensating DROP failed too"),
            "premise: the double fault was reached, so the line under test was \
             written at all; log={text}"
        );
        assert!(
            !text.contains(SHORT_ROOT_PW),
            "the FE root password reached the compensation log: {text}"
        );
        assert!(
            text.contains("Access denied"),
            "premise: the message survives redaction apart from the credential, \
             so the scrub map is what removed it; log={text}"
        );
    }

    /// D2 + D8 (checkpoint 3): an engine deleted while the loop runs stops it,
    /// and the attachments it never reached are never touched — not `applied`,
    /// and not `failed`, because nothing was attempted. Before SP3b this
    /// checkpoint could only be asserted by counting `engine_still_current(`
    /// in the function's own source text.
    #[tokio::test]
    async fn deleting_the_engine_mid_loop_stops_the_bootstrap_and_leaves_the_rest_alone() {
        let Some((st, fake, prov)) = test_state_with_fake_fe().await else {
            eprintln!("skipping deleting_the_engine_mid_loop_stops_the_bootstrap_and_leaves_the_rest_alone: DATABASE_URL not set");
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (wh, _s) = seed_sr_warehouse(&st, &db).await;
        let engine = seed_sr_engine(&st).await;
        prov.set_status(ready_fe_status(&engine.name));

        let sfx = Uuid::new_v4().simple().to_string();
        let names = [format!("a_{sfx}"), format!("m_{sfx}"), format!("z_{sfx}")];
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        seed_pending_attachments(&st, &engine, wh.id, &refs).await;

        // The fake deletes the engine row the moment the FIRST catalog is
        // created, so the loop's next checkpoint is the one under test.
        let store = st.engine_mgmt.clone();
        let engine_id = engine.id;
        fake.on_create(move |_name| {
            let store = store.clone();
            Box::pin(async move { store.delete_engine(engine_id).await.unwrap() })
        });

        spawn_starrocks_catalog_wire(
            st.clone(),
            engine.id,
            engine.name.clone(),
            engine.namespace.clone(),
            None,
        )
        .await
        .expect("bootstrap task did not panic");

        assert_eq!(
            fake.created(),
            vec![names[0].clone()],
            "the loop stopped at the next catalog; sent={:?}",
            fake.executed()
        );
        // The rows themselves are gone with the engine (the attachment FK
        // cascades), so what the FE was told is the checkpoint's observable
        // effect: exactly one CREATE, and nothing at all about m_ or z_.
        assert_eq!(
            fake.executed()
                .iter()
                .filter(|s| s.starts_with("CREATE EXTERNAL CATALOG"))
                .count(),
            1
        );
        for skipped in &names[1..] {
            assert!(
                !fake.executed().iter().any(|s| s.contains(skipped.as_str())),
                "nothing was said about {skipped}: {:?}",
                fake.executed()
            );
        }
    }

    /// D7 inside the loop: an attachment DETACHED while the bootstrap was
    /// creating its catalog leaves the FE holding a catalog egret has
    /// forgotten, and nothing on a StarRocks FE prunes it — so the convergence
    /// drops what it just created. The row's disappearance is what
    /// `set_apply_applied` reports (`NotFound`), and it is the ONLY write
    /// failure that may compensate: an ordinary database error leaves the row
    /// in place, and dropping then would destroy a catalog the registry still
    /// claims.
    #[tokio::test]
    async fn a_catalog_whose_row_vanished_mid_bootstrap_is_dropped_again() {
        let Some((st, fake, prov)) = test_state_with_fake_fe().await else {
            eprintln!("skipping a_catalog_whose_row_vanished_mid_bootstrap_is_dropped_again: DATABASE_URL not set");
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (wh, _s) = seed_sr_warehouse(&st, &db).await;
        let engine = seed_sr_engine(&st).await;
        prov.set_status(ready_fe_status(&engine.name));

        let sfx = Uuid::new_v4().simple().to_string();
        let names = [format!("a_{sfx}"), format!("z_{sfx}")];
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let ids = seed_pending_attachments(&st, &engine, wh.id, &refs).await;

        // The first catalog is detached the instant its CREATE reaches the FE.
        let registry = st.engine_mgmt.registry();
        let engine_id = engine.id;
        let gone = ids[&names[0]];
        fake.on_create(move |_name| {
            let registry = registry.clone();
            Box::pin(async move {
                registry.delete_attachment(engine_id, gone).await.unwrap();
            })
        });

        spawn_starrocks_catalog_wire(
            st.clone(),
            engine.id,
            engine.name.clone(),
            engine.namespace.clone(),
            None,
        )
        .await
        .expect("bootstrap task did not panic");

        assert!(
            fake.executed()
                .iter()
                .any(|s| s.starts_with("DROP CATALOG IF EXISTS") && s.contains(names[0].as_str())),
            "the created-then-forgotten catalog is dropped again: {:?}",
            fake.executed()
        );
        assert!(
            fake.catalog(&names[0]).is_none(),
            "and it is really gone from the FE"
        );
        // The DROP is the whole compensation: the loop goes on, and the
        // catalog whose row survived is applied and NOT dropped.
        assert_eq!(
            attachment_status(&st, engine.id, ids[&names[1]])
                .await
                .apply_status,
            "applied"
        );
        assert!(
            fake.catalog(&names[1]).is_some(),
            "a catalog whose row is intact must never be compensated"
        );
    }

    /// One catalog's DDL failing does not abandon the others, and the FE's
    /// words reach `apply_error` scrubbed.
    ///
    /// It does NOT claim to pin D6's statement/secret guarantee: the message
    /// this fake injects never contained the statement, so an assertion that
    /// the statement is absent could not fail for the reason it named (fix
    /// round 1, F5 — that line is deleted rather than dressed up). The real
    /// cover is Task 4's
    /// `an_fe_error_stored_on_the_attachment_carries_no_ddl_and_no_secret`
    /// (`tests/routes.rs`), which injects an FE error that DOES echo the
    /// statement and the secret and asserts neither survives.
    #[tokio::test]
    async fn a_failing_catalog_is_flagged_and_the_loop_goes_on() {
        let Some((st, fake, prov)) = test_state_with_fake_fe().await else {
            eprintln!(
                "skipping a_failing_catalog_is_flagged_and_the_loop_goes_on: DATABASE_URL not set"
            );
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (wh, _s) = seed_sr_warehouse(&st, &db).await;
        let engine = seed_sr_engine(&st).await;
        prov.set_status(ready_fe_status(&engine.name));

        let sfx = Uuid::new_v4().simple().to_string();
        let names = [format!("a_{sfx}"), format!("m_{sfx}"), format!("z_{sfx}")];
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let ids = seed_pending_attachments(&st, &engine, wh.id, &refs).await;
        fake.fail_catalog(
            &names[1],
            "Getting analyzing error. Detail message: unsupported type",
        );

        spawn_starrocks_catalog_wire(
            st.clone(),
            engine.id,
            engine.name.clone(),
            engine.namespace.clone(),
            None,
        )
        .await
        .expect("bootstrap task did not panic");

        assert_eq!(
            attachment_status(&st, engine.id, ids[&names[0]])
                .await
                .apply_status,
            "applied"
        );
        let m = attachment_status(&st, engine.id, ids[&names[1]]).await;
        assert_eq!(m.apply_status, "failed");
        let err = m.apply_error.expect("the failure is recorded on the row");
        assert!(err.contains("unsupported type"), "{err}");
        assert!(attachment_status(&st, engine.id, ids[&names[1]])
            .await
            .applied_at
            .is_none());
        assert_eq!(
            attachment_status(&st, engine.id, ids[&names[2]])
                .await
                .apply_status,
            "applied",
            "a failure before it must not cost the last catalog its apply"
        );
    }

    /// Checkpoints 1 and 2: an FE that never reports ready, and a root login
    /// that never succeeds, each end the task having executed no catalog DDL —
    /// and leave every attachment `pending`, because nothing was attempted.
    /// Both loops are bounded at `MAX_POLLS` × `POLL_INTERVAL`, which the test
    /// build shrinks to one poll; otherwise this would take five minutes per
    /// case, which is a test nobody runs.
    #[tokio::test]
    async fn an_fe_that_never_becomes_ready_writes_nothing() {
        let Some((st, fake, prov)) = test_state_with_fake_fe().await else {
            eprintln!(
                "skipping an_fe_that_never_becomes_ready_writes_nothing: DATABASE_URL not set"
            );
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (wh, _s) = seed_sr_warehouse(&st, &db).await;
        let engine = seed_sr_engine(&st).await;
        let name = format!("a_{}", Uuid::new_v4().simple());
        let ids = seed_pending_attachments(&st, &engine, wh.id, &[name.as_str()]).await;

        // (a) status reports a pod that is not an FE.
        prov.set_status(egret_engine_mgmt_api::EngineStatus {
            phase: "provisioning".into(),
            message: None,
            pods: vec![egret_engine_mgmt_api::PodStatus {
                name: "sr-cn-0".into(),
                phase: "Running".into(),
                ready: true,
            }],
        });
        spawn_starrocks_catalog_wire(
            st.clone(),
            engine.id,
            engine.name.clone(),
            engine.namespace.clone(),
            None,
        )
        .await
        .expect("bootstrap task did not panic");
        assert!(
            fake.executed().is_empty(),
            "no statement before the FE is ready: {:?}",
            fake.executed()
        );
        assert!(prov.calls() > 0, "the poll really ran");
        assert_eq!(
            attachment_status(&st, engine.id, ids[&name])
                .await
                .apply_status,
            "pending",
            "an unreachable FE leaves `pending`, not `failed`: nothing was attempted"
        );

        // (b) FE ready, but root login never succeeds.
        fake.set_fail(Some("Access denied for user 'root' (using password: YES)"));
        prov.set_status(ready_fe_status(&engine.name));
        spawn_starrocks_catalog_wire(
            st.clone(),
            engine.id,
            engine.name.clone(),
            engine.namespace.clone(),
            None,
        )
        .await
        .expect("bootstrap task did not panic");
        assert!(
            fake.created().is_empty(),
            "no catalog before root can log in: {:?}",
            fake.executed()
        );
        assert_eq!(
            attachment_status(&st, engine.id, ids[&name])
                .await
                .apply_status,
            "pending",
            "an FE that will not take the root login leaves `pending` too"
        );
    }

    /// Make this engine stop being `current` — a status flip to `deleting`,
    /// which `bootstrap_target_is_current` refuses exactly like a delete, but
    /// WITHOUT removing the row. That difference is the point: a deleted row
    /// makes the bootstrap's own `get_engine` bail before the checkpoint under
    /// test, so only a flip can isolate a checkpoint that sits after it (fix
    /// round 1, F1).
    async fn stop_being_current(store: &crate::EngineMgmtStore, engine_id: Uuid) {
        store
            .set_engine_status(engine_id, crate::engine_status::DELETING)
            .await
            .expect("flip the engine to deleting");
    }

    /// Checkpoint 2 — the one guarding the root-login probe
    /// (`engines.rs`, the `for poll in 0..MAX_POLLS` loop after `fe_target`).
    ///
    /// The cluster poll is the only thing that happens between checkpoint 1
    /// and checkpoint 2, so the engine is flipped to `deleting` from inside
    /// `status()`. The probe is a `query`, not an `execute`, so `executed()`
    /// cannot see it — `queried()` is what makes this checkpoint observable at
    /// all, and its absence is why deleting this checkpoint left the whole
    /// suite green before this round.
    #[tokio::test]
    async fn the_root_login_checkpoint_stops_a_bootstrap_whose_engine_is_being_deleted() {
        let Some((st, fake, prov)) = test_state_with_fake_fe().await else {
            eprintln!("skipping the_root_login_checkpoint_stops_a_bootstrap_whose_engine_is_being_deleted: DATABASE_URL not set");
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (wh, _s) = seed_sr_warehouse(&st, &db).await;
        let engine = seed_sr_engine(&st).await;
        prov.set_status(ready_fe_status(&engine.name));
        let name = format!("a_{}", Uuid::new_v4().simple());
        let ids = seed_pending_attachments(&st, &engine, wh.id, &[name.as_str()]).await;

        let store = st.engine_mgmt.clone();
        let engine_id = engine.id;
        prov.on_status(move || {
            let store = store.clone();
            Box::pin(async move { stop_being_current(&store, engine_id).await })
        });

        spawn_starrocks_catalog_wire(
            st.clone(),
            engine.id,
            engine.name.clone(),
            engine.namespace.clone(),
            None,
        )
        .await
        .expect("bootstrap task did not panic");

        assert!(
            fake.queried().is_empty(),
            "the root-login probe presents the FE root password; it must not be \
             attempted against an engine that is on its way out: {:?}",
            fake.queried()
        );
        assert!(fake.executed().is_empty(), "{:?}", fake.executed());
        assert_eq!(
            attachment_status(&st, engine.id, ids[&name])
                .await
                .apply_status,
            "pending"
        );
    }

    /// Checkpoint 4 — the one before `CREATE SECURITY INTEGRATION`.
    ///
    /// One attachment, and the engine is flipped while its catalog is being
    /// created: checkpoint 3 has already passed for that attachment, the loop
    /// then ends, and checkpoint 4 is the next thing between the bootstrap and
    /// a write. Nothing of the delegation half may reach the FE.
    #[tokio::test]
    async fn the_delegation_checkpoint_stops_before_the_security_integration() {
        let Some((st, fake, prov)) = test_state_with_fake_fe().await else {
            eprintln!("skipping the_delegation_checkpoint_stops_before_the_security_integration: DATABASE_URL not set");
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (wh, _s) = seed_sr_warehouse(&st, &db).await;
        let engine = seed_sr_delegating_engine(&st, "CERTPEM").await;
        prov.set_status(ready_fe_status(&engine.name));
        let name = format!("a_{}", Uuid::new_v4().simple());
        seed_pending_attachments(&st, &engine, wh.id, &[name.as_str()]).await;

        let store = st.engine_mgmt.clone();
        let engine_id = engine.id;
        fake.on_create(move |_name| {
            let store = store.clone();
            Box::pin(async move { stop_being_current(&store, engine_id).await })
        });

        spawn_starrocks_catalog_wire(
            st.clone(),
            engine.id,
            engine.name.clone(),
            engine.namespace.clone(),
            Some(test_delegation()),
        )
        .await
        .expect("bootstrap task did not panic");

        assert_eq!(fake.created().len(), 1, "premise: the catalog ran first");
        assert!(
            !fake
                .executed()
                .iter()
                .any(|s| s.contains("SECURITY INTEGRATION")),
            "no delegation statement may reach an engine that is being deleted: {:?}",
            fake.executed()
        );
    }

    /// Checkpoint 5 — the one before the repair `ALTER SECURITY INTEGRATION`.
    ///
    /// The FE answers the `CREATE` with `already exists`, which is the branch
    /// that leads to the repair; the engine stops being current while that
    /// very statement is in flight (`on_execute` fires BEFORE the fake decides
    /// to reject it, which is the whole reason that hook exists). The `ALTER`
    /// must not be sent.
    #[tokio::test]
    async fn the_repair_checkpoint_stops_before_the_alter() {
        let Some((st, fake, prov)) = test_state_with_fake_fe().await else {
            eprintln!(
                "skipping the_repair_checkpoint_stops_before_the_alter: DATABASE_URL not set"
            );
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (_wh, _s) = seed_sr_warehouse(&st, &db).await;
        let engine = seed_sr_delegating_engine(&st, "CERTPEM").await;
        prov.set_status(ready_fe_status(&engine.name));
        fake.fail_matching(
            "CREATE SECURITY INTEGRATION",
            "Getting analyzing error. Detail message: security integration 'egret_oidc' already exists.",
        );
        // Premise: the injected FE message is recognized as already-exists —
        // checked independently of the run below, because if `is_already_exists`
        // ever regressed to `false` here, the general `Err(e)` arm would take
        // over, stop at its OWN checkpoint before ever reaching checkpoint 5,
        // and still send no `ALTER` — leaving every assertion below green while
        // no longer exercising checkpoint 5 at all.
        let probe_fe = crate::starrocks_ddl::fe_target(&st, &engine)
            .await
            .expect("fe target");
        assert!(
            matches!(
                st.starrocks_ddl
                    .execute(&probe_fe, "CREATE SECURITY INTEGRATION premise_probe")
                    .await,
                Err(e) if e.is_already_exists()
            ),
            "premise: the injected FE failure must be recognized as already-exists"
        );
        fake.clear_executed();
        let store = st.engine_mgmt.clone();
        let engine_id = engine.id;
        fake.on_execute(move |sql| {
            let store = store.clone();
            Box::pin(async move {
                if sql.contains("CREATE SECURITY INTEGRATION") {
                    stop_being_current(&store, engine_id).await;
                }
            })
        });

        spawn_starrocks_catalog_wire(
            st.clone(),
            engine.id,
            engine.name.clone(),
            engine.namespace.clone(),
            Some(test_delegation()),
        )
        .await
        .expect("bootstrap task did not panic");

        let sent = fake.executed();
        assert!(
            sent.iter()
                .any(|s| s.contains("CREATE SECURITY INTEGRATION")),
            "premise: the CREATE was attempted and refused as already existing: {sent:?}"
        );
        assert!(
            !sent
                .iter()
                .any(|s| s.contains("ALTER SECURITY INTEGRATION")),
            "the repair must not be sent to an engine that is being deleted: {sent:?}"
        );
    }

    /// Checkpoint 6 — the one inside the loop over the remaining delegation
    /// statements (`ADMIN SET FRONTEND CONFIG`, `CREATE USER`,
    /// `GRANT IMPERSONATE`).
    ///
    /// Same hook, but the integration SUCCEEDS this time, so the repair branch
    /// is not taken and the next thing between the bootstrap and a write is
    /// checkpoint 6. None of the three may be sent.
    #[tokio::test]
    async fn the_remaining_delegation_statements_stop_at_their_own_checkpoint() {
        let Some((st, fake, prov)) = test_state_with_fake_fe().await else {
            eprintln!("skipping the_remaining_delegation_statements_stop_at_their_own_checkpoint: DATABASE_URL not set");
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (_wh, _s) = seed_sr_warehouse(&st, &db).await;
        let engine = seed_sr_delegating_engine(&st, "CERTPEM").await;
        prov.set_status(ready_fe_status(&engine.name));
        let store = st.engine_mgmt.clone();
        let engine_id = engine.id;
        fake.on_execute(move |sql| {
            let store = store.clone();
            Box::pin(async move {
                if sql.contains("CREATE SECURITY INTEGRATION") {
                    stop_being_current(&store, engine_id).await;
                }
            })
        });

        spawn_starrocks_catalog_wire(
            st.clone(),
            engine.id,
            engine.name.clone(),
            engine.namespace.clone(),
            Some(test_delegation()),
        )
        .await
        .expect("bootstrap task did not panic");

        let sent = fake.executed();
        assert!(
            sent.iter()
                .any(|s| s.contains("CREATE SECURITY INTEGRATION")),
            "premise: the integration was created: {sent:?}"
        );
        for stopped in [
            "ADMIN SET FRONTEND CONFIG",
            "CREATE USER IF NOT EXISTS",
            "GRANT IMPERSONATE",
        ] {
            assert!(
                !sent.iter().any(|s| s.contains(stopped)),
                "{stopped} must not be sent to an engine that is being deleted: {sent:?}"
            );
        }
    }

    /// The delegation bundle every bootstrap test passes. Its
    /// `service_password` is what the delegation half scrubs FE errors
    /// against, alongside the engine's root password.
    fn test_delegation() -> StarRocksDelegationBootstrap {
        StarRocksDelegationBootstrap {
            jwks_url: "https://idp/jwks".into(),
            issuer: "https://idp/realms/egret".into(),
            audience: "engine".into(),
            service_user: "egret_delegator".into(),
            service_password: "pw".into(),
        }
    }

    /// Checkpoints 4-6: the delegation half still runs, and `already exists`
    /// still repairs — driven through the fake runner instead of a real MySQL
    /// port, which is what
    /// `spawn_starrocks_catalog_wire_keeps_all_six_guard_checkpoints` existed
    /// to approximate.
    #[tokio::test]
    async fn the_delegation_bootstrap_still_creates_then_repairs() {
        let Some((st, fake, prov)) = test_state_with_fake_fe().await else {
            eprintln!("skipping the_delegation_bootstrap_still_creates_then_repairs: DATABASE_URL not set");
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (_wh, _s) = seed_sr_warehouse(&st, &db).await;
        let engine = seed_sr_delegating_engine(&st, "CERTPEM").await;
        prov.set_status(ready_fe_status(&engine.name));
        let delegation = test_delegation();

        // First run: the integration is created, the user and grant follow.
        spawn_starrocks_catalog_wire(
            st.clone(),
            engine.id,
            engine.name.clone(),
            engine.namespace.clone(),
            Some(delegation.clone()),
        )
        .await
        .expect("bootstrap task did not panic");
        let sent = fake.executed();
        assert!(
            sent.iter()
                .any(|s| s.contains("CREATE SECURITY INTEGRATION egret_oidc")),
            "{sent:?}"
        );
        assert!(
            sent.iter().any(|s| s.contains("CREATE USER IF NOT EXISTS")),
            "{sent:?}"
        );
        assert!(
            sent.iter().any(|s| s.contains("GRANT IMPERSONATE")),
            "{sent:?}"
        );

        // Second run against an FE that already has it: the repair ALTER runs,
        // so a changed issuer takes effect instead of being lost to
        // `already exists`.
        //
        // The message deliberately carries a SQL-escaped apostrophe (fix round
        // 1, F2). That shape is unpairable by the redaction pipeline, so the
        // EXCERPT is withheld in full — and the commit under review decided
        // repair-vs-warn by asking `contains("already exists")` of exactly that
        // withheld string, which answers `false`. The plain-quoting shape is
        // covered by `already_exists_is_decided_for_the_caller_from_the_raw_message`;
        // this is the shape that has to be driven through the real call site,
        // because the bug was in the call site, not in the predicate.
        fake.fail_matching(
            "CREATE SECURITY INTEGRATION",
            r#"Getting analyzing error. Detail message: security integration 'egret\'s oidc' already exists."#,
        );
        fake.clear_executed();
        spawn_starrocks_catalog_wire(
            st.clone(),
            engine.id,
            engine.name.clone(),
            engine.namespace.clone(),
            Some(delegation),
        )
        .await
        .expect("bootstrap task did not panic");
        assert!(
            fake.executed()
                .iter()
                .any(|s| s.contains("ALTER SECURITY INTEGRATION egret_oidc")),
            "{:?}",
            fake.executed()
        );

        // D6: the delegation statements now share the catalog path's
        // `FeTarget`, so the engine's pinned certificate reaches all of them —
        // and the root-login probe, which carries the root password.
        assert_eq!(fake.last_target().tls_cert_pem.as_deref(), Some("CERTPEM"));
    }

    /// A stored StarRocks engine that has opted into delegation and carries a
    /// pinned coordinator certificate — the shape whose FE really serves TLS,
    /// and so the one whose every statement must go out over it (D6).
    async fn seed_sr_delegating_engine(st: &CpState, cert: &str) -> Engine {
        let spec = serde_json::json!({
            "imageVersion": DEFAULT_ENGINE_IMAGES.starrocks_fe_image,
            "starrocksConfig": {
                "feImage": DEFAULT_ENGINE_IMAGES.starrocks_fe_image,
                "cnImage": DEFAULT_ENGINE_IMAGES.starrocks_cn_image,
                "feReplicas": 1,
                "cnReplicas": 1,
                // `tlsEnabled` deliberately OMITTED: it defaults to on, which
                // is what `build_starrocks_provision_req` provisions.
                "delegation": { "mode": "jwt" },
            },
        });
        let engine = st
            .engine_mgmt
            .create_engine(
                &format!("sr-{}", Uuid::new_v4().simple()),
                "starrocks",
                "egret-engines",
                &spec,
                None,
            )
            .await
            .expect("create engine row");
        let keystore = st
            .secrets
            .create(
                None,
                &format!("engine-tls-{}", engine.id),
                None,
                &format!("{cert}KEYPEM"),
            )
            .await
            .expect("create tls secret");
        st.engine_mgmt
            .set_tls(engine.id, keystore, cert)
            .await
            .expect("pin the cert");
        st.engine_mgmt
            .get_engine(engine.id)
            .await
            .expect("get_engine")
            .expect("engine present")
    }

    #[test]
    fn preserve_trino_delegation_mode_keeps_the_stored_mode_only_when_omitted() {
        let stored = serde_json::json!({ "trinoConfig": { "delegationMode": "jwt" } });
        let mut omitted = serde_json::json!({ "trinoConfig": { "worker": {} } });
        preserve_trino_delegation_mode(&mut omitted, &stored);
        assert_eq!(omitted["trinoConfig"]["delegationMode"], "jwt");
        let mut explicit = serde_json::json!({ "trinoConfig": { "delegationMode": "none" } });
        preserve_trino_delegation_mode(&mut explicit, &stored);
        assert_eq!(explicit["trinoConfig"]["delegationMode"], "none");
        let mut from_legacy = serde_json::json!({ "trinoConfig": {} });
        preserve_trino_delegation_mode(&mut from_legacy, &serde_json::json!({}));
        assert!(from_legacy["trinoConfig"].get("delegationMode").is_none());
    }

    /// P3 (StarRocks): a rejected apply restores the stored spec and is a 502.
    #[tokio::test]
    async fn update_starrocks_engine_failure_restores_spec() {
        let Some(mut st) = test_state().await else {
            eprintln!(
                "skipping update_starrocks_engine_failure_restores_spec: DATABASE_URL not set"
            );
            return;
        };
        st.engine_provisioner = Some(std::sync::Arc::new(FailingUpdateProvisioner));
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (wh, _secret) = seed_sr_warehouse(&st, &db).await;
        let engine = seed_sr_engine(&st).await;
        // The warehouse this engine leads with comes from its PRIMARY
        // attachment (SP3c-2 Task 5), which `create_starrocks_engine` writes
        // and the bare row fixture does not.
        attach_primary_warehouse_catalog(&st, &engine, wh.id).await;
        let body = sr_update_body(Some(StarRocksConfigBody {
            fe_replicas: Some(3),
            ..Default::default()
        }));
        let err = update_starrocks_engine(st.clone(), engine.clone(), body)
            .await
            .err()
            .expect("the apply failure must surface");
        assert!(matches!(err, CpError::ClusterApplyFailed(_)), "got {err:?}");
        let reloaded = st.engine_mgmt.get_engine(engine.id).await.unwrap().unwrap();
        assert_eq!(reloaded.spec, engine.spec, "stored spec restored");
    }

    /// Without Kubernetes an update is refused before anything is persisted.
    #[tokio::test]
    async fn update_starrocks_engine_without_kubernetes_changes_nothing() {
        let Some(st) = test_state().await else {
            eprintln!(
                "skipping update_starrocks_engine_without_kubernetes_changes_nothing: DATABASE_URL not set"
            );
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (_wh, _secret) = seed_sr_warehouse(&st, &db).await;
        let engine = seed_sr_engine(&st).await;
        let body = sr_update_body(Some(StarRocksConfigBody {
            fe_replicas: Some(3),
            ..Default::default()
        }));
        let err = update_starrocks_engine(st.clone(), engine.clone(), body)
            .await
            .err()
            .map(|e| e.to_string());
        assert!(
            matches!(&err, Some(m) if m.contains("kubernetes not configured")),
            "got {err:?}"
        );
        let reloaded = st.engine_mgmt.get_engine(engine.id).await.unwrap().unwrap();
        assert_eq!(reloaded.spec["starrocksConfig"]["feReplicas"], 1);
    }

    /// Carried finding: `EngineMgmtStore::update_engine` must not clobber a
    /// concurrent DELETE's `deleting` marker. Simulates a PUT that read the
    /// engine as `provisioning` (the status seeded on it) racing a DELETE that
    /// has since moved it to `deleting`: the stale-status write must match zero
    /// rows, leaving BOTH the status and the spec exactly as the DELETE left
    /// them.
    #[tokio::test]
    async fn update_engine_cannot_move_a_row_out_of_deleting() {
        let Some(st) = test_state().await else {
            eprintln!(
                "skipping update_engine_cannot_move_a_row_out_of_deleting: DATABASE_URL not set"
            );
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (_wh, _secret) = seed_sr_warehouse(&st, &db).await;
        let engine = seed_sr_engine(&st).await;
        assert_eq!(engine.status, crate::engine_status::PROVISIONING);

        // Concurrent DELETE flips the row to `deleting`.
        st.engine_mgmt
            .set_engine_status(engine.id, crate::engine_status::DELETING)
            .await
            .unwrap();

        // A stale PUT, still holding the pre-delete `provisioning` it read
        // earlier, tries to persist a new spec.
        let new_spec = serde_json::json!({ "starrocksConfig": { "feReplicas": 9 } });
        let matched = st
            .engine_mgmt
            .update_engine(engine.id, &new_spec, &engine.status)
            .await
            .unwrap();
        assert!(!matched, "the stale-status write must match zero rows");

        let reloaded = st.engine_mgmt.get_engine(engine.id).await.unwrap().unwrap();
        assert_eq!(
            reloaded.status,
            crate::engine_status::DELETING,
            "status must not be clobbered back"
        );
        assert_eq!(
            reloaded.spec, engine.spec,
            "spec must not be overwritten either"
        );
    }

    /// Handler-level version of the above, through `update_starrocks_engine`
    /// itself: a PUT whose `Engine` snapshot was read before a concurrent
    /// status change (simulated with a direct `set_engine_status` between the
    /// seed and the call, standing in for a delete that raced the handler's own
    /// read) must 409 BEFORE touching the provisioner at all — never build a
    /// request or call `update`, which on a real cluster would re-create the CR
    /// of an engine whose row is already gone.
    #[tokio::test]
    async fn update_starrocks_engine_conflict_stops_before_the_provisioner() {
        let Some(mut st) = test_state().await else {
            eprintln!(
                "skipping update_starrocks_engine_conflict_stops_before_the_provisioner: DATABASE_URL not set"
            );
            return;
        };
        let counting = std::sync::Arc::new(CountingProvisioner::default());
        st.engine_provisioner = Some(counting.clone());
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (wh, _secret) = seed_sr_warehouse(&st, &db).await;
        let engine = seed_sr_engine(&st).await;
        // The warehouse this engine leads with comes from its PRIMARY
        // attachment (SP3c-2 Task 5), which `create_starrocks_engine` writes
        // and the bare row fixture does not.
        attach_primary_warehouse_catalog(&st, &engine, wh.id).await;

        // Simulate a concurrent change (e.g. a delete) that landed after this
        // handler call's `Engine` was already read.
        st.engine_mgmt
            .set_engine_status(engine.id, crate::engine_status::DELETING)
            .await
            .unwrap();

        let body = sr_update_body(Some(StarRocksConfigBody {
            fe_replicas: Some(9),
            ..Default::default()
        }));
        let err = update_starrocks_engine(st.clone(), engine.clone(), body)
            .await
            .err()
            .expect("a lost race must surface as an error, not a silent 200");
        assert!(matches!(err, CpError::Conflict(_)), "got {err:?}");
        assert!(
            err.to_string().contains("retry"),
            "message should tell the caller to retry; got {err}"
        );

        let reloaded = st.engine_mgmt.get_engine(engine.id).await.unwrap().unwrap();
        assert_eq!(reloaded.spec, engine.spec, "stored spec untouched");
        assert_eq!(
            counting.updates.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "the provisioner must never be called on a lost race"
        );
    }

    /// Trino counterpart of `update_starrocks_engine_conflict_stops_before_the_provisioner`
    /// above, through `update_trino_engine` (the ordering is separately written
    /// code for the two engine kinds, so it needs its own regression guard): a
    /// PUT whose `Engine` snapshot was read before a concurrent status change
    /// (simulated with a direct `set_engine_status` between the seed and the
    /// call) must 409 BEFORE touching the provisioner at all.
    #[tokio::test]
    async fn update_trino_engine_conflict_stops_before_the_provisioner() {
        let Some(mut st) = test_state().await else {
            eprintln!(
                "skipping update_trino_engine_conflict_stops_before_the_provisioner: DATABASE_URL not set"
            );
            return;
        };
        let counting = std::sync::Arc::new(CountingProvisioner::default());
        st.engine_provisioner = Some(counting.clone());
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (wh, _secret) = seed_sr_warehouse(&st, &db).await;
        let engine = st
            .engine_mgmt
            .create_engine(
                &format!("trino-{}", Uuid::new_v4().simple()),
                "trino",
                DEFAULT_NAMESPACE,
                &serde_json::json!({ "imageVersion": "483", "trinoConfig": { "delegationMode": "jwt" } }),
                None,
            )
            .await
            .expect("create engine row");

        // Simulate a concurrent change (e.g. a delete) that landed after this
        // handler call's `Engine` was already read.
        st.engine_mgmt
            .set_engine_status(engine.id, crate::engine_status::DELETING)
            .await
            .unwrap();

        let body = CreateEngineBody {
            name: engine.name.clone(),
            warehouse_id: Some(wh.id),
            catalogs: Vec::new(),
            image_version: None,
            coordinator_replicas: 2,
            worker_replicas: 5,
            cpu_min: "1".into(),
            cpu_max: "2".into(),
            memory: "4Gi".into(),
            trino_config: None,
            authz_service_id: None,
            kind: "trino".into(),
            starrocks: None,
            tls_cert_secret_ref: None,
        };
        let err = update_trino_engine(st.clone(), engine.clone(), body)
            .await
            .err()
            .expect("a lost race must surface as an error, not a silent 200");
        assert!(matches!(err, CpError::Conflict(_)), "got {err:?}");
        assert!(
            err.to_string().contains("retry"),
            "message should tell the caller to retry; got {err}"
        );

        let reloaded = st.engine_mgmt.get_engine(engine.id).await.unwrap().unwrap();
        assert_eq!(reloaded.spec, engine.spec, "stored spec untouched");
        assert_eq!(
            counting.updates.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "the provisioner must never be called on a lost race"
        );
    }

    /// Critical (SP1 Part B final review, C1) — the guard `apply_built` itself
    /// carries. `rotate_engine_secrets`, `add_catalog`, `remove_catalog` and
    /// `update_catalog` all fetch the engine, do real work (build a request,
    /// sometimes mint a client), and only THEN call `apply_built`, which is
    /// server-side apply: called on an engine a concurrent DELETE has already
    /// torn down, it would RE-CREATE every object DELETE just removed. This
    /// proves the re-read `apply_built` does immediately before the provisioner
    /// call — independent of any earlier per-route check — refuses once the row
    /// is `deleting`, never calling the provisioner. Simulated the same way the
    /// CAS tests above simulate a race: build the request from a `provisioning`
    /// engine snapshot, then flip the row to `deleting` (as a concurrent DELETE
    /// would) before calling `apply_built`.
    #[tokio::test]
    async fn apply_built_refuses_when_the_row_is_deleting() {
        let Some(st) = test_state().await else {
            eprintln!(
                "skipping apply_built_refuses_when_the_row_is_deleting: DATABASE_URL not set"
            );
            return;
        };
        let counting = std::sync::Arc::new(CountingProvisioner::default());
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (_wh, _secret) = seed_sr_warehouse(&st, &db).await;
        let engine = st
            .engine_mgmt
            .create_engine(
                &format!("trino-{}", Uuid::new_v4().simple()),
                "trino",
                DEFAULT_NAMESPACE,
                &serde_json::json!({ "imageVersion": "483", "trinoConfig": { "delegationMode": "jwt" } }),
                None,
            )
            .await
            .expect("create engine row");
        assert_eq!(engine.status, crate::engine_status::PROVISIONING);

        let built = build_provision_req(&st, &engine)
            .await
            .expect("build request");

        // Concurrent DELETE lands after this snapshot was read (the same
        // interleaving the four routes are exposed to between their own
        // `get_engine` and their `apply_built` call).
        st.engine_mgmt
            .set_engine_status(engine.id, crate::engine_status::DELETING)
            .await
            .unwrap();

        let err = apply_built(&st, counting.as_ref(), &engine, &built, ApplyOp::Update)
            .await
            .expect_err(
                "a lost race against a delete must surface as an error, not a silent apply",
            );
        assert!(
            matches!(err, ApplyBuiltError::Conflict),
            "expected Conflict"
        );
        assert_eq!(
            counting.updates.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "the provisioner must never be called once the row is deleting"
        );
    }

    /// An engine whose spec names no image is an unforeseen row (0185 wrote one
    /// into every provisioned engine). The build must say which engine and
    /// field, and must NOT fall back to the current default, which would upgrade
    /// it behind its operator's back.
    #[tokio::test]
    async fn building_an_engine_with_no_stored_image_is_an_error_not_the_default() {
        let Some(st) = test_state().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (wh, _secret) = seed_sr_warehouse(&st, &db).await;
        let trino = st
            .engine_mgmt
            .create_engine(
                &format!("trino-noimg-{}", Uuid::new_v4().simple()),
                "trino",
                DEFAULT_NAMESPACE,
                &serde_json::json!({}),
                None,
            )
            .await
            .expect("create engine row");
        let err = build_provision_req(&st, &trino)
            .await
            .err()
            .expect("refused");
        let msg = err.to_string();
        assert!(
            msg.contains(&trino.name) && msg.contains("imageVersion"),
            "{msg}"
        );

        let sr = st
            .engine_mgmt
            .create_engine(
                &format!("sr-noimg-{}", Uuid::new_v4().simple()),
                "starrocks",
                "egret-engines",
                &serde_json::json!({ "starrocksConfig": { "feImage": "starrocks/fe-ubuntu:4.1.6" } }),
                None,
            )
            .await
            .expect("create engine row");
        let err = build_starrocks_provision_req(&st, &sr, &wh)
            .await
            .err()
            .expect("refused");
        let msg = err.to_string();
        assert!(
            msg.contains(&sr.name) && msg.contains("starrocksConfig.cnImage"),
            "{msg}"
        );
    }

    /// N1 (round-2 review, from the C1 fix): the opposite direction of the
    /// test above. `apply_built`'s guard must NOT demand the row still be at
    /// the exact status the caller observed — only that it isn't gone or
    /// `deleting`. A concurrent `GET /engines/{id}` recording
    /// `provisioning → running` (the detail page polls it every 5 seconds) is
    /// NOT a reason to refuse a legitimate apply landing in the same window —
    /// reachable in the first minute of an engine's life, exactly when an
    /// admin is likely to add a catalog. Same simulated-race shape as the test
    /// above, but the concurrent change is `provisioning → running` instead of
    /// a delete, and the apply must SUCCEED (provisioner called once).
    #[tokio::test]
    async fn apply_built_succeeds_when_the_row_merely_advanced_to_running() {
        let Some(st) = test_state().await else {
            eprintln!(
                "skipping apply_built_succeeds_when_the_row_merely_advanced_to_running: DATABASE_URL not set"
            );
            return;
        };
        let counting = std::sync::Arc::new(CountingProvisioner::default());
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (_wh, _secret) = seed_sr_warehouse(&st, &db).await;
        let engine = st
            .engine_mgmt
            .create_engine(
                &format!("trino-{}", Uuid::new_v4().simple()),
                "trino",
                DEFAULT_NAMESPACE,
                &serde_json::json!({ "imageVersion": "483", "trinoConfig": { "delegationMode": "jwt" } }),
                None,
            )
            .await
            .expect("create engine row");
        assert_eq!(engine.status, crate::engine_status::PROVISIONING);

        let built = build_provision_req(&st, &engine)
            .await
            .expect("build request");

        // A concurrent GET /engines/{id} records the live `running` status
        // (the same transition `get_engine`'s guard writes) in the window
        // between this snapshot's read and the apply below.
        st.engine_mgmt
            .transition_engine_status(
                engine.id,
                crate::engine_status::PROVISIONING,
                crate::engine_status::RUNNING,
            )
            .await
            .unwrap();

        apply_built(&st, counting.as_ref(), &engine, &built, ApplyOp::Update)
            .await
            .expect("an apply must succeed when the row merely advanced to running");
        assert_eq!(
            counting.updates.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the provisioner must be called exactly once"
        );
    }

    /// N2 (round-2 review, from the C1 fix): on the `Conflict` path
    /// `apply_built` must discard a freshly-minted internal/monitor pair
    /// rather than leak two unreferenced secret-store rows — nothing was
    /// applied at that point, so nothing else can be relying on it.
    #[tokio::test]
    async fn apply_built_discards_a_fresh_pair_on_conflict() {
        let Some(st) = test_state().await else {
            eprintln!(
                "skipping apply_built_discards_a_fresh_pair_on_conflict: DATABASE_URL not set"
            );
            return;
        };
        let counting = std::sync::Arc::new(CountingProvisioner::default());
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (_wh, _secret) = seed_sr_warehouse(&st, &db).await;
        let engine = st
            .engine_mgmt
            .create_engine(
                &format!("trino-{}", Uuid::new_v4().simple()),
                "trino",
                DEFAULT_NAMESPACE,
                &serde_json::json!({ "imageVersion": "483", "trinoConfig": { "delegationMode": "jwt" } }),
                None,
            )
            .await
            .expect("create engine row");

        // Force a fresh pair to be minted (mirrors what `rotate_engine_secrets`
        // asks `build_provision_req_with` for).
        let built = build_provision_req_with(
            &st,
            &engine,
            BuildOpts {
                rotate_internal_secrets: true,
                ..BuildOpts::default()
            },
        )
        .await
        .expect("build request");
        let (internal_id, monitor_id) = {
            let secrets = built.internal.as_ref().expect("a fresh pair was minted");
            (secrets.internal_id, secrets.monitor_id)
        };

        // Concurrent DELETE lands before the apply's own re-check.
        st.engine_mgmt
            .set_engine_status(engine.id, crate::engine_status::DELETING)
            .await
            .unwrap();

        apply_built(&st, counting.as_ref(), &engine, &built, ApplyOp::Update)
            .await
            .expect_err("must refuse once the row is deleting");
        assert_eq!(
            counting.updates.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "the provisioner must never be called"
        );
        for (label, id) in [("internal", internal_id), ("monitor", monitor_id)] {
            assert!(
                matches!(
                    st.secrets.resolve_plaintext(None, id).await,
                    Err(egret_secrets::SecretsError::NotFound)
                ),
                "the unapplied {label} secret must be discarded, not leaked"
            );
        }
    }

    /// Critical (SP1 Part B final review, C1) — handler level, the review's
    /// suggested regression: seed an engine, mark it `deleting` directly (as a
    /// concurrent DELETE would just before these land), then call
    /// `rotate_engine_secrets` and `catalog_apply::attach_to_engine` (SP3c-2
    /// Task 4 deleted `add_catalog`; `attach_to_engine` is the shared function
    /// behind the kept `POST /engines/{id}/catalogs/{cid}` AND the deleted
    /// route both used, and carries the identical `DELETING` guard as its
    /// first check — before the catalog argument is even examined, so a
    /// syntactically-valid-but-never-persisted `CatalogRow` reaches it here).
    /// Both must reject and the fake port must record no update.
    #[tokio::test]
    async fn rotate_and_attach_catalog_reject_a_deleting_engine() {
        let Some(mut st) = test_state().await else {
            eprintln!(
                "skipping rotate_and_attach_catalog_reject_a_deleting_engine: DATABASE_URL not set"
            );
            return;
        };
        let counting = std::sync::Arc::new(CountingProvisioner::default());
        st.engine_provisioner = Some(counting.clone());
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (_wh, _secret) = seed_sr_warehouse(&st, &db).await;
        let engine = st
            .engine_mgmt
            .create_engine(
                &format!("trino-{}", Uuid::new_v4().simple()),
                "trino",
                DEFAULT_NAMESPACE,
                &serde_json::json!({ "imageVersion": "483", "trinoConfig": { "delegationMode": "jwt" } }),
                None,
            )
            .await
            .expect("create engine row");

        // A DELETE has already moved the row before either request lands.
        st.engine_mgmt
            .set_engine_status(engine.id, crate::engine_status::DELETING)
            .await
            .unwrap();

        let principal = egret_core::context::Principal {
            user_id: egret_core::ids::UserId::new(),
            email: "admin@egret.local".into(),
            platform_roles: vec!["platform_admin".into()],
            permissions: vec![],
            group_ids: vec![],
            impersonator: None,
        };

        let err = rotate_engine_secrets(State(st.clone()), Auth(principal), Path(engine.id))
            .await
            .expect_err("rotate on a deleting engine must fail");
        assert!(matches!(err, CpError::Conflict(_)), "got {err:?}");

        // Unlike the handlers, `attach_to_engine` takes an already-loaded
        // `&Engine` rather than re-fetching by id — re-read it after the
        // status update, or its `DELETING` check sees the stale in-memory
        // snapshot from before this test moved the row.
        let deleting_engine = st
            .engine_mgmt
            .get_engine(engine.id)
            .await
            .expect("reload engine")
            .expect("engine still exists");
        // A `CatalogRow` that was never inserted — irrelevant, since the
        // `DELETING` check fires before `attach_to_engine` looks at it.
        let dummy_catalog = egret_catalog_registry::CatalogRow {
            id: Uuid::new_v4(),
            name: "extra".into(),
            kind: "postgresql".into(),
            config: serde_json::json!({}),
            advanced: serde_json::json!({}),
            description: None,
            warehouse_id: None,
            storage_connection_id: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let err =
            crate::catalog_apply::attach_to_engine(&st, &deleting_engine, &dummy_catalog, None)
                .await
                .expect_err("attach_to_engine on a deleting engine must fail");
        assert!(
            matches!(
                err,
                crate::catalog_apply::CatalogOpError::Http(CpError::Conflict(_))
            ),
            "got {err:?}"
        );

        assert_eq!(
            counting.updates.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "the provisioner must never be called on a deleting engine"
        );
    }

    /// A PUT that bumps `feReplicas` + sets `feResources` re-persists those into
    /// the stored spec while preserving the untouched `cnReplicas`/`feImage`.
    /// The fake provisioner accepts the apply, so the spec stays persisted.
    #[tokio::test]
    async fn update_starrocks_engine_rescales() {
        let Some(mut st) = test_state().await else {
            eprintln!("skipping update_starrocks_engine_rescales: DATABASE_URL not set");
            return;
        };
        st.engine_provisioner = Some(std::sync::Arc::new(NoopProvisioner));
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (wh, _secret) = seed_sr_warehouse(&st, &db).await;
        let engine = seed_sr_engine(&st).await;
        // The warehouse this engine leads with comes from its PRIMARY
        // attachment (SP3c-2 Task 5), which `create_starrocks_engine` writes
        // and the bare row fixture does not.
        attach_primary_warehouse_catalog(&st, &engine, wh.id).await;

        let body = sr_update_body(Some(StarRocksConfigBody {
            fe_replicas: Some(2),
            fe_resources: Some(serde_json::json!({ "memRequest": "2Gi", "memLimit": "4Gi" })),
            ..Default::default()
        }));
        if let Err(e) = update_starrocks_engine(st.clone(), engine.clone(), body).await {
            panic!("update failed: {e}");
        }

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
        assert_eq!(cfg["feImage"], DEFAULT_ENGINE_IMAGES.starrocks_fe_image); // untouched ⇒ preserved
    }

    /// An update must never leak storage coords into the stored spec, and the
    /// re-assembled provision request must still derive `storage_path` from the
    /// warehouse (`<bucket>/starrocks/<engine>`), unchanged by the config edit.
    #[tokio::test]
    async fn update_starrocks_engine_preserves_storage() {
        let Some(mut st) = test_state().await else {
            eprintln!("skipping update_starrocks_engine_preserves_storage: DATABASE_URL not set");
            return;
        };
        st.engine_provisioner = Some(std::sync::Arc::new(NoopProvisioner));
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (wh, _secret) = seed_sr_warehouse(&st, &db).await;
        let engine = seed_sr_engine(&st).await;
        // The warehouse this engine leads with comes from its PRIMARY
        // attachment (SP3c-2 Task 5), which `create_starrocks_engine` writes
        // and the bare row fixture does not.
        attach_primary_warehouse_catalog(&st, &engine, wh.id).await;

        let body = sr_update_body(Some(StarRocksConfigBody {
            cn_replicas: Some(4),
            ..Default::default()
        }));
        if let Err(e) = update_starrocks_engine(st.clone(), engine.clone(), body).await {
            panic!("update failed: {e}");
        }

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
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (wh, _secret) = seed_sr_warehouse(&st, &db).await;
        let engine = seed_sr_engine(&st).await;
        // The warehouse this engine leads with comes from its PRIMARY
        // attachment (SP3c-2 Task 5), which `create_starrocks_engine` writes
        // and the bare row fixture does not.
        attach_primary_warehouse_catalog(&st, &engine, wh.id).await;

        let req = build_provision_req(&st, &engine)
            .await
            .expect("build_provision_req")
            .req;
        assert_eq!(req.kind, egret_engine_mgmt_api::EngineKind::StarRocks);
        assert!(req.starrocks.is_some());
        assert!(req.catalogs.is_empty());
    }

    /// P1: internal secrets are random, stay OFF the row until committed, are
    /// reused when present, and a discarded or superseded generation is deleted.
    /// The monitor presents the legacy value exactly until a commit.
    #[tokio::test]
    async fn internal_secrets_are_pending_until_committed() {
        let Some(st) = test_state().await else {
            eprintln!(
                "skipping internal_secrets_are_pending_until_committed: DATABASE_URL not set"
            );
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (_wh, _secret) = seed_sr_warehouse(&st, &db).await;
        let engine = st
            .engine_mgmt
            .create_engine(
                &format!("trino-{}", Uuid::new_v4().simple()),
                "trino",
                "egret-engines",
                &serde_json::json!({ "imageVersion": "483", "trinoConfig": {} }),
                None,
            )
            .await
            .expect("create engine row");
        let not_found = |r: Result<String, egret_secrets::SecretsError>| {
            matches!(r, Err(egret_secrets::SecretsError::NotFound))
        };

        let first = resolve_internal_secrets(&st, &engine, false)
            .await
            .expect("mint");
        assert!(first.fresh);
        assert_ne!(first.internal, first.monitor);
        {
            use base64::Engine as _;
            let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(&first.internal)
                .expect("url-safe base64");
            assert_eq!(raw.len(), 32);
        }
        let row = st.engine_mgmt.get_engine(engine.id).await.unwrap().unwrap();
        assert!(row.internal_secret_id.is_none() && row.monitor_secret_id.is_none());
        assert_eq!(
            monitor_password_for(&st, &row).await.unwrap(),
            legacy_monitor_password(engine.id),
            "uncommitted ⇒ the pods still have the legacy password"
        );

        commit_internal_secrets(&st, &row, &first, Supersede::Delete)
            .await
            .expect("commit");
        let row = st.engine_mgmt.get_engine(engine.id).await.unwrap().unwrap();
        assert_eq!(row.internal_secret_id, Some(first.internal_id));
        assert_eq!(row.monitor_secret_id, Some(first.monitor_id));
        assert_eq!(
            monitor_password_for(&st, &row).await.unwrap(),
            first.monitor
        );

        let reused = resolve_internal_secrets(&st, &row, false)
            .await
            .expect("reuse");
        assert!(!reused.fresh);
        assert_eq!(reused.internal, first.internal);

        let rotated = resolve_internal_secrets(&st, &row, true)
            .await
            .expect("rotate");
        assert!(rotated.fresh);
        assert_ne!(rotated.internal, first.internal);
        discard_internal_secrets(&st, &rotated).await;
        assert!(not_found(
            st.secrets
                .resolve_plaintext(None, rotated.internal_id)
                .await
        ));
        assert!(not_found(
            st.secrets.resolve_plaintext(None, rotated.monitor_id).await
        ));
        assert_eq!(
            st.engine_mgmt
                .get_engine(engine.id)
                .await
                .unwrap()
                .unwrap()
                .monitor_secret_id,
            Some(first.monitor_id),
            "a discard never touches the row"
        );

        let rotated = resolve_internal_secrets(&st, &row, true)
            .await
            .expect("rotate again");
        commit_internal_secrets(&st, &row, &rotated, Supersede::Delete)
            .await
            .expect("commit rotation");
        assert!(not_found(
            st.secrets.resolve_plaintext(None, first.internal_id).await
        ));
        assert!(not_found(
            st.secrets.resolve_plaintext(None, first.monitor_id).await
        ));
    }

    /// F2 fix: two concurrent applies on the same NULL-secret-ids engine each
    /// mint their own pair; only the FIRST commit may write the row — the
    /// second, still holding the stale (both-`None`) snapshot it minted
    /// against, must not clobber the winner. The loser's pair is left
    /// unreferenced (never deleted — see `commit_internal_secrets`'s doc
    /// comment for why), and the next apply on the now-current row reuses the
    /// winner.
    #[tokio::test]
    async fn concurrent_applies_do_not_clobber_each_others_commit() {
        let Some(st) = test_state().await else {
            eprintln!(
                "skipping concurrent_applies_do_not_clobber_each_others_commit: DATABASE_URL not set"
            );
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (_wh, _secret) = seed_sr_warehouse(&st, &db).await;
        let engine = st
            .engine_mgmt
            .create_engine(
                &format!("trino-{}", Uuid::new_v4().simple()),
                "trino",
                "egret-engines",
                &serde_json::json!({ "imageVersion": "483", "trinoConfig": {} }),
                None,
            )
            .await
            .expect("create engine row");

        // Two concurrent builds both read the same NULL row.
        let a = resolve_internal_secrets(&st, &engine, false)
            .await
            .expect("mint a");
        let b = resolve_internal_secrets(&st, &engine, false)
            .await
            .expect("mint b");
        assert_ne!(a.internal_id, b.internal_id);

        // The first apply's commit wins.
        commit_internal_secrets(&st, &engine, &a, Supersede::Delete)
            .await
            .expect("commit a");
        let after_a = st.engine_mgmt.get_engine(engine.id).await.unwrap().unwrap();
        assert_eq!(after_a.internal_secret_id, Some(a.internal_id));
        assert_eq!(after_a.monitor_secret_id, Some(a.monitor_id));

        // F2 round 2 regression guard: A's `Supersede::Delete` commit deletes
        // ONLY the pair its own compare-and-set displaced (here: nothing —
        // `engine`'s pre-build ids were both `None`) and must NEVER reach for
        // B's already-minted, not-yet-committed pair just because it happens
        // to share this engine's secret-name prefix. Round 1 of this fix used
        // a prefix-wide sweep on a successful commit and a re-review
        // reproduced exactly this: A's sweep deleted B's pair before B ever
        // got to commit it, even though B's apply might be the one that lands
        // on the cluster last. Assert B's pair is untouched.
        assert!(
            st.secrets
                .resolve_plaintext(None, b.internal_id)
                .await
                .is_ok(),
            "A's commit must not delete B's still-in-flight pair"
        );
        assert!(
            st.secrets
                .resolve_plaintext(None, b.monitor_id)
                .await
                .is_ok(),
            "A's commit must not delete B's still-in-flight pair"
        );

        // The second apply's commit, still holding the STALE (both-`None`)
        // `engine` snapshot, must not overwrite it.
        commit_internal_secrets(&st, &engine, &b, Supersede::Delete)
            .await
            .expect("commit b (loses the race, but does not error)");
        let after_b = st.engine_mgmt.get_engine(engine.id).await.unwrap().unwrap();
        assert_eq!(
            after_b.internal_secret_id,
            Some(a.internal_id),
            "the winner's pair must survive"
        );
        assert_eq!(after_b.monitor_secret_id, Some(a.monitor_id));

        // Losing the race must not delete B's own pair either — it is left
        // orphaned (unreferenced by the row) but resolvable, exactly like a
        // `Supersede::Keep`'d pair; only engine delete (Task 15) reclaims it.
        assert!(
            st.secrets
                .resolve_plaintext(None, b.internal_id)
                .await
                .is_ok(),
            "a losing commit must not delete its own pair"
        );
        assert!(
            st.secrets
                .resolve_plaintext(None, b.monitor_id)
                .await
                .is_ok(),
            "a losing commit must not delete its own pair"
        );

        // The next apply on the now-current row reuses the committed winner.
        let reused = resolve_internal_secrets(&st, &after_b, false)
            .await
            .expect("reuse");
        assert!(!reused.fresh);
        assert_eq!(reused.internal_id, a.internal_id);
        assert_eq!(reused.monitor_id, a.monitor_id);
    }

    /// Minor fix: an admin (or anything else) deleting a recorded secret
    /// out-of-band must not brick every future apply on the engine — the next
    /// `resolve_internal_secrets` mints a fresh pair instead of propagating
    /// `SecretsError::NotFound` forever.
    #[tokio::test]
    async fn resolve_internal_secrets_remints_when_a_stored_secret_is_gone() {
        let Some(st) = test_state().await else {
            eprintln!(
                "skipping resolve_internal_secrets_remints_when_a_stored_secret_is_gone: DATABASE_URL not set"
            );
            return;
        };
        let dsn = egret_test_db::url().await.unwrap();
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        let (_wh, _secret) = seed_sr_warehouse(&st, &db).await;
        let engine = st
            .engine_mgmt
            .create_engine(
                &format!("trino-{}", Uuid::new_v4().simple()),
                "trino",
                "egret-engines",
                &serde_json::json!({ "imageVersion": "483", "trinoConfig": {} }),
                None,
            )
            .await
            .expect("create engine row");
        let first = resolve_internal_secrets(&st, &engine, false)
            .await
            .expect("mint");
        commit_internal_secrets(&st, &engine, &first, Supersede::Delete)
            .await
            .expect("commit");
        let row = st.engine_mgmt.get_engine(engine.id).await.unwrap().unwrap();

        // Delete the internal secret out-of-band (not through this module).
        st.secrets
            .delete(None, first.internal_id)
            .await
            .expect("delete internal secret directly");

        let reminted = resolve_internal_secrets(&st, &row, false)
            .await
            .expect("must remint instead of erroring");
        assert!(reminted.fresh);
        assert_ne!(reminted.internal_id, first.internal_id);
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

    /// The repair statement carries every property that can drift, and carries
    /// them under the same escaping as the `CREATE`.
    ///
    /// `type` is deliberately absent: it is the integration's kind, not a
    /// setting. Asserting its absence is what stops a future "just re-send
    /// everything" edit from turning a repair into a rejected statement.
    #[test]
    fn starrocks_jwt_repair_sql_sets_every_drifting_property() {
        let sql = starrocks_jwt_integration_repair_sql(
            "https://kc/realms/egret/protocol/openid-connect/certs",
            "https://egret.example",
            "engine",
        );
        assert!(sql.starts_with("ALTER SECURITY INTEGRATION egret_oidc SET ("));
        assert!(
            sql.contains(r#""jwks_url"="https://kc/realms/egret/protocol/openid-connect/certs""#)
        );
        assert!(sql.contains(r#""principal_field"="preferred_username""#));
        assert!(sql.contains(r#""required_issuer"="https://egret.example""#));
        assert!(sql.contains(r#""required_audience"="engine""#));
        assert!(
            !sql.contains(r#""type"="#),
            "type is not a settable property; sql={sql}"
        );
    }

    /// Same DDL-injection posture as the `CREATE`: this runs as `root` on the
    /// FE, so a repair path that forgot to escape would be a hole the create
    /// path does not have.
    #[test]
    fn starrocks_jwt_repair_sql_escapes_ddl_injection() {
        let evil = r#"kc" ) ; DROP DATABASE x ; -- "#;
        let sql = starrocks_jwt_integration_repair_sql("https://kc/jwks", evil, "aud");
        assert!(
            !sql.contains(r#"kc" ) ; DROP DATABASE x"#),
            "unescaped issuer breakout present; sql={sql}"
        );
        assert!(
            sql.contains(r#"required_issuer"="kc\" ) ; DROP DATABASE x ; -- ""#),
            "issuer not escaped; sql={sql}"
        );
    }

    /// A malicious/free-form issuer/audience/jwks_url must not break out of
    /// the double-quoted property value: this statement runs as `root` on the
    /// FE and none of its inputs is charset-validated upstream.
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
    /// the literal early. StarRocks is MySQL-dialect, so `\` must be escaped
    /// too — a quote-only escape leaves `\'` able to break out and inject DDL
    /// that runs as `root`.
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

        // Backslash payload: the old quote-only escape rendered
        // `… 'u\'x'@'%' IDENTIFIED BY 'p\' OR 1=1 -- '`, where StarRocks reads
        // `\'` as an escaped quote, so the literal runs on and `OR 1=1 -- `
        // lands as SQL. Both `\` and `'` must be escaped.
        let bs_sql = starrocks_create_service_user_sql(r"u\'x", r"p\' OR 1=1 -- ");
        assert_eq!(
            bs_sql,
            r#"CREATE USER IF NOT EXISTS 'u\\''x'@'%' IDENTIFIED BY 'p\\'' OR 1=1 -- '"#
        );
        assert!(
            !bs_sql.contains(r"'u\'x'"),
            "backslash-blind username escape present; sql={bs_sql}"
        );
        assert!(
            !bs_sql.contains(r"'p\' OR"),
            "backslash-blind password escape present; sql={bs_sql}"
        );
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
