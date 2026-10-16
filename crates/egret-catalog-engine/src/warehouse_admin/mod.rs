//! Control-plane admin router for the embedded catalog: warehouses,
//! catalog-roles, grants, Catalog Clients, and warehouse browse/admin ops.
//! Moved out of `egret-controlplane` into the Catalog seam (E3d-3); the router
//! is still nested at `/api/control-plane` in `egret-server` (FE URLs
//! unchanged). Every route is gated on `egret_core::authz::require(&*st.pdp, &p,
//! "catalog_registry:*")` (platform_admin bypasses inside the PDP) and operates
//! over the egret RBAC store (`CatalogStore`). SP3c-2 retired the router's
//! original `warehouse:*` gate: migration 0153 re-ran 0148's grant and deleted
//! the old keys, so `catalog_registry:*` alone now admits anyone who could
//! reach these routes before. NOTE: warehouse creation here records the
//! egret-side row; provisioning the lakekeeper warehouse itself is done
//! through the [`WarehouseProvisioner`] seam, wired in the composition root
//! (egret-bin).
//!
//! OpenAPI (E0·P4b-4): every handler carries a `#[utoipa::path]` with the FULL
//! mounted path (`/api/control-plane` + the internal route), since utoipa does
//! not compose with axum `nest()`. The wire DTOs derive [`utoipa::ToSchema`] and
//! the co-located [`CatalogAdminApi`] doc struct re-exports them; errors use the
//! shared [`egret_core::http::ErrorBody`]. `RoleDto` is emitted as
//! `CatalogRoleDto` (see its `#[schema(as = …)]`) so it does not clobber the
//! control-plane RBAC `RoleDto` in the merged doc (schema names are global).

mod error;

use axum::extract::{FromRef, Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use egret_auth::principal::Auth;
use egret_auth::session::SessionService;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;
use uuid::Uuid;

use egret_core::http::ErrorBody;

use crate::{CatalogStore, GrantInput, NewClient};
pub use error::CatalogHttpError;

/// State for the admin catalog router. The `CatalogStore` + storage-connection
/// registry + secret store are all in-crate/downstream deps; the
/// `WarehouseProvisioner`/`WarehouseBrowser` are composition-root seams wired in
/// egret-bin (`None` in tests, which record the egret row without a lakekeeper
/// counterpart).
#[derive(Clone)]
pub struct CatalogAdminState {
    /// Embedded-catalog store: warehouses + catalog RBAC (roles/grants/clients).
    pub store: CatalogStore,
    /// Global storage-connection registry (`storage.storage_connections`, E3a).
    pub storage_registry: egret_storage::StorageRegistry,
    /// Named secret layer, used in GLOBAL scope (`None` workspace) here.
    pub secrets: egret_secrets::WorkspaceSecretStore,
    /// Provisions the embedded lakekeeper warehouse on create (wired in egret-bin).
    pub provisioner: Option<Arc<dyn WarehouseProvisioner>>,
    /// Reads catalog structure from the embedded lakekeeper (wired in egret-bin).
    pub browser: Option<Arc<dyn WarehouseBrowser>>,
    /// Session service backing the `Auth` extractor.
    pub sessions: SessionService,
    /// Policy decision point: every route gates on `egret_core::authz::require`
    /// (B1b-2). `platform_admin` still bypasses inside the PDP.
    pub pdp: std::sync::Arc<dyn egret_core::authz::Pdp>,
    /// Recompiles a catalog's implicit assignment grant / sweeps its stale
    /// compiled policy on teardown, in the SAME transaction as the write
    /// (SP4a Task 16, D3 defects 2 and 3). `None` leaves both call sites
    /// behaved exactly as before this task (every test in this crate, and any
    /// deployment that predates it) — same contract
    /// `egret_engine_mgmt::EngineMgmtState::perm_compiler` already has. The
    /// concrete implementation is wired in the composition root; see
    /// `egret_catalog_registry::CatalogAssignmentCompiler` for why it is a
    /// port and not a direct call.
    pub perm_compiler: Option<Arc<dyn egret_catalog_registry::CatalogAssignmentCompiler>>,
}

impl FromRef<CatalogAdminState> for SessionService {
    fn from_ref(s: &CatalogAdminState) -> Self {
        s.sessions.clone()
    }
}

/// Build the admin catalog router (to be nested at `/api/control-plane`).
pub fn catalog_admin_router() -> Router<CatalogAdminState> {
    router()
}

/// S3 config handed to the [`WarehouseProvisioner`] to create the lakekeeper
/// warehouse (secret already resolved to plaintext).
pub struct ProvisionS3 {
    pub bucket: String,
    pub region: String,
    pub endpoint: Option<String>,
    pub path_style: bool,
    pub access_key: String,
    pub secret_key: String,
    pub key_prefix: Option<String>,
}

/// Provisions the embedded lakekeeper warehouse for a just-created egret
/// `warehouses` row. Implemented in the composition root (egret-bin), which holds
/// the lakekeeper `ApiContext`. Returns the lakekeeper warehouse id, or an error.
#[async_trait::async_trait]
pub trait WarehouseProvisioner: Send + Sync {
    async fn provision(
        &self,
        name: &str,
        s3: &ProvisionS3,
    ) -> Result<String, crate::CatalogEngineError>;
}

/// Catalog browse result rows (control-plane DTOs; camelCase on the wire).
pub struct BrowseNamespace {
    pub parts: Vec<String>,
    pub id: Option<uuid::Uuid>,
    pub protected: bool,
}
pub struct BrowseTabular {
    pub name: String,
    pub kind: String, // "table" | "view"
    pub id: Option<uuid::Uuid>,
    pub protected: bool,
}
/// A soft-deleted table/view awaiting purge (recoverable via undrop).
pub struct BrowseDeletedTabular {
    pub id: uuid::Uuid,
    pub name: String,
    pub namespace: Vec<String>,
    pub kind: String, // "table" | "view"
    pub deleted_at: chrono::DateTime<chrono::Utc>,
    pub expiration: chrono::DateTime<chrono::Utc>,
}
pub struct BrowseColumn {
    pub name: String,
    pub type_: String,
    pub required: bool,
}
pub struct BrowsePartition {
    pub name: String,
    pub source_column: String,
    pub transform: String,
}
pub struct BrowseSort {
    pub column: String,
    pub transform: String,
    pub direction: String,
    pub null_order: String,
}
pub struct BrowseSnapshot {
    pub snapshot_id: i64,
    pub parent_snapshot_id: Option<i64>,
    pub timestamp_ms: i64,
    pub operation: String,
    pub summary: std::collections::BTreeMap<String, String>,
}
/// Rich, UI-friendly table detail (schema + partition/sort + location + props + snapshots).
pub struct BrowseTableDetail {
    pub location: String,
    pub format_version: u8,
    pub last_updated_ms: i64,
    pub current_snapshot_id: Option<i64>,
    pub snapshot_count: usize,
    pub columns: Vec<BrowseColumn>,
    pub partition_fields: Vec<BrowsePartition>,
    pub sort_fields: Vec<BrowseSort>,
    pub properties: std::collections::BTreeMap<String, String>,
    pub snapshots: Vec<BrowseSnapshot>,
}

/// Reads catalog structure (namespaces/tables/schema) from the embedded
/// lakekeeper. Implemented in egret-bin over the lakekeeper `ApiContext`
/// (like `WarehouseProvisioner`). All args key by the lakekeeper warehouse id.
#[async_trait::async_trait]
pub trait WarehouseBrowser: Send + Sync {
    async fn namespaces(
        &self,
        lakekeeper_warehouse_id: &str,
    ) -> Result<Vec<BrowseNamespace>, crate::CatalogEngineError>;
    async fn tabulars(
        &self,
        lakekeeper_warehouse_id: &str,
        namespace: &[String],
    ) -> Result<Vec<BrowseTabular>, crate::CatalogEngineError>;
    async fn schema(
        &self,
        lakekeeper_warehouse_id: &str,
        namespace: &[String],
        table: &str,
    ) -> Result<BrowseTableDetail, crate::CatalogEngineError>;
    /// Current-schema columns of a view.
    async fn view_columns(
        &self,
        lakekeeper_warehouse_id: &str,
        namespace: &[String],
        view: &str,
    ) -> Result<Vec<BrowseColumn>, crate::CatalogEngineError>;
    /// Soft-deleted tables/views awaiting purge.
    async fn deleted_tabulars(
        &self,
        lakekeeper_warehouse_id: &str,
    ) -> Result<Vec<BrowseDeletedTabular>, crate::CatalogEngineError>;
    /// Recover a soft-deleted table (`kind`="table") or view (`kind`="view").
    async fn undrop(
        &self,
        lakekeeper_warehouse_id: &str,
        kind: &str,
        id: uuid::Uuid,
    ) -> Result<(), crate::CatalogEngineError>;
    /// Set the protected flag on a `target` ("table"/"view"/"namespace") by id.
    async fn set_protection(
        &self,
        lakekeeper_warehouse_id: &str,
        target: &str,
        id: uuid::Uuid,
        protected: bool,
    ) -> Result<bool, crate::CatalogEngineError>;
    /// Read the warehouse's admin config (status / soft-delete TTL / statistics).
    async fn config(
        &self,
        lakekeeper_warehouse_id: &str,
    ) -> Result<BrowseWarehouseConfig, crate::CatalogEngineError>;
    /// Activate/deactivate the warehouse.
    async fn set_active(
        &self,
        lakekeeper_warehouse_id: &str,
        active: bool,
    ) -> Result<(), crate::CatalogEngineError>;
    /// Set soft-delete retention (`Some(seconds)`) or hard delete (`None`).
    async fn set_delete_profile(
        &self,
        lakekeeper_warehouse_id: &str,
        seconds: Option<i64>,
    ) -> Result<(), crate::CatalogEngineError>;
    /// Set the allowed Iceberg format versions.
    async fn set_format_versions(
        &self,
        lakekeeper_warehouse_id: &str,
        versions: Vec<u8>,
    ) -> Result<(), crate::CatalogEngineError>;
    /// Rotate the S3 access-key credential.
    async fn rotate_credential(
        &self,
        lakekeeper_warehouse_id: &str,
        access_key: &str,
        secret_key: &str,
    ) -> Result<(), crate::CatalogEngineError>;
    /// Search tables/views across the warehouse by name.
    async fn search(
        &self,
        lakekeeper_warehouse_id: &str,
        query: &str,
    ) -> Result<Vec<BrowseSearchResult>, crate::CatalogEngineError>;
    /// List recent background (GC) tasks on the warehouse.
    async fn tasks(
        &self,
        lakekeeper_warehouse_id: &str,
    ) -> Result<Vec<BrowseTask>, crate::CatalogEngineError>;
}

/// Warehouse admin config (status / soft-delete TTL / latest statistics).
pub struct BrowseWarehouseConfig {
    pub active: bool,
    pub delete_profile_seconds: Option<i64>,
    pub num_tables: i64,
    pub num_views: i64,
    pub stats_updated_ms: i64,
}
/// A table/view name-search match.
pub struct BrowseSearchResult {
    pub namespace: Vec<String>,
    pub name: String,
    pub kind: String,
}
/// A background (GC) task summary.
pub struct BrowseTask {
    pub queue: String,
    pub entity: Option<String>,
    pub status: String,
}

fn router() -> Router<CatalogAdminState> {
    Router::new()
        .route("/warehouses", get(list_warehouses).post(create_warehouse))
        .route(
            "/warehouses/{id}",
            get(get_warehouse).delete(delete_warehouse),
        )
        .route("/warehouses/{id}/roles", get(list_roles).post(create_role))
        .route("/roles/{role_id}", delete(delete_role))
        .route(
            "/warehouses/{id}/clients",
            get(list_clients).post(create_client),
        )
        .route("/roles/{role_id}/grants", get(list_grants).post(add_grant))
        .route("/grants/{grant_id}", delete(revoke_grant))
        .route("/clients/{id}", delete(revoke_client))
        .route("/warehouses/{id}/namespaces", get(browse_namespaces))
        .route(
            "/warehouses/{id}/namespaces/{ns}/tables",
            get(browse_tables),
        )
        .route(
            "/warehouses/{id}/namespaces/{ns}/tables/{table}",
            get(browse_schema),
        )
        .route(
            "/warehouses/{id}/namespaces/{ns}/views/{view}",
            get(browse_view),
        )
        .route("/warehouses/{id}/deleted-tabulars", get(browse_deleted))
        .route("/warehouses/{id}/undrop", post(undrop_tabular))
        .route("/warehouses/{id}/protection", put(set_protection))
        .route("/warehouses/{id}/config", get(warehouse_config))
        .route("/warehouses/{id}/status", put(set_warehouse_status))
        .route(
            "/warehouses/{id}/delete-profile",
            put(set_delete_profile_route),
        )
        .route("/warehouses/{id}/format-policy", put(set_format_policy))
        .route(
            "/warehouses/{id}/storage-credential",
            put(rotate_credential),
        )
        .route("/warehouses/{id}/search", get(search_tabulars))
        .route("/warehouses/{id}/tasks", get(list_tasks))
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct WarehouseDto {
    id: Uuid,
    name: String,
    workspace_id: Uuid,
    storage_conn_id: Uuid,
    lakekeeper_warehouse_id: Option<String>,
}
impl From<crate::Warehouse> for WarehouseDto {
    fn from(w: crate::Warehouse) -> Self {
        WarehouseDto {
            id: w.id,
            name: w.name,
            workspace_id: w.workspace_id,
            storage_conn_id: w.storage_conn_id,
            lakekeeper_warehouse_id: w.lakekeeper_warehouse_id,
        }
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct CreateWarehouseBody {
    name: String,
    workspace_id: Uuid,
    storage_connection_id: Uuid,
}

#[utoipa::path(
    get,
    path = "/api/control-plane/warehouses",
    tag = "control-plane-catalog",
    responses(
        (status = 200, description = "Embedded-catalog warehouses (egret rows)", body = Vec<WarehouseDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_warehouses(
    State(st): State<CatalogAdminState>,
    Auth(p): Auth,
) -> Result<Json<Vec<WarehouseDto>>, CatalogHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:read").await?;
    Ok(Json(
        st.store
            .list_warehouses()
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/warehouses",
    tag = "control-plane-catalog",
    request_body = CreateWarehouseBody,
    responses(
        (status = 201, description = "Warehouse created (egret row + lakekeeper warehouse when a provisioner is wired) with its registry catalog, assigned to the workspace", body = WarehouseDto),
        (status = 400, description = "Unknown storage connection or invalid input", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:create permission", body = ErrorBody),
        (status = 409, description = "A warehouse with this name already exists", body = ErrorBody),
        (status = 502, description = "Embedded catalog unreachable while provisioning", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn create_warehouse(
    State(st): State<CatalogAdminState>,
    Auth(p): Auth,
    Json(b): Json<CreateWarehouseBody>,
) -> Result<(StatusCode, Json<WarehouseDto>), CatalogHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:create").await?;
    let w = st
        .store
        .create_warehouse(&b.name, b.workspace_id, b.storage_connection_id)
        .await?;

    // Provision the actual lakekeeper warehouse (so data ops work). If it fails,
    // roll back the egret row — a warehouse with no lakekeeper counterpart is useless.
    if let Some(prov) = &st.provisioner {
        match provision_lakekeeper(&st, &b.name, b.storage_connection_id, prov.as_ref()).await {
            Ok(lk_id) => st.store.set_lakekeeper_warehouse_id(w.id, &lk_id).await?,
            Err(e) => {
                let _ = st.store.delete_warehouse(w.id).await;
                return Err(e);
            }
        }
    }

    let w = st
        .store
        .get_warehouse(w.id)
        .await?
        .ok_or(CatalogHttpError::NotFound)?;

    // SP3: the warehouse is a registry catalog, assigned to this workspace. A
    // failure is not fatal — `ensure_warehouse_catalog` is idempotent, and
    // engine create calls it again on every attach to this warehouse.
    if let Err(e) = st
        .store
        .registry()
        .ensure_warehouse_catalog_with_compiler(
            &egret_catalog_registry::WarehouseIdentity {
                id: w.id,
                name: &w.name,
                workspace_id: w.workspace_id,
            },
            None,
            st.perm_compiler.as_deref(),
        )
        .await
    {
        tracing::warn!(warehouse = %w.name, error = %e, "warehouse created but its registry catalog was not");
    }

    Ok((StatusCode::CREATED, Json(w.into())))
}

/// Resolve the storage connection (+ its secret) and provision the lakekeeper warehouse.
async fn provision_lakekeeper(
    st: &CatalogAdminState,
    name: &str,
    storage_conn_id: Uuid,
    prov: &dyn WarehouseProvisioner,
) -> Result<String, CatalogHttpError> {
    let sc = st
        .storage_registry
        .get_storage_conn(storage_conn_id)
        .await?
        .ok_or_else(|| CatalogHttpError::BadRequest("unknown storage connection".into()))?;
    let secret_key = match sc.secret_ref {
        // Storage-connection secrets are GLOBAL (None scope), per P2.
        Some(ref_id) => st
            .secrets
            .resolve_plaintext(None, ref_id)
            .await
            .map_err(|e| CatalogHttpError::BadRequest(format!("resolve storage secret: {e}")))?,
        None => String::new(),
    };
    let s3 = ProvisionS3 {
        bucket: sc.bucket,
        region: sc.region,
        endpoint: sc.endpoint,
        path_style: sc.path_style,
        access_key: sc.access_key,
        secret_key,
        key_prefix: sc.key_prefix,
    };
    prov.provision(name, &s3)
        .await
        .map_err(CatalogHttpError::from)
}

#[utoipa::path(
    get,
    path = "/api/control-plane/warehouses/{id}",
    tag = "control-plane-catalog",
    params(("id" = Uuid, Path, description = "Warehouse id")),
    responses(
        (status = 200, description = "The warehouse", body = WarehouseDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:read permission", body = ErrorBody),
        (status = 404, description = "No such warehouse", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn get_warehouse(
    State(st): State<CatalogAdminState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<WarehouseDto>, CatalogHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:read").await?;
    let w = st
        .store
        .get_warehouse(id)
        .await?
        .ok_or(CatalogHttpError::NotFound)?;
    Ok(Json(w.into()))
}

#[utoipa::path(
    delete,
    path = "/api/control-plane/warehouses/{id}",
    tag = "control-plane-catalog",
    params(("id" = Uuid, Path, description = "Warehouse id")),
    responses(
        (status = 204, description = "Warehouse deleted"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:delete permission", body = ErrorBody),
        (status = 409, description = "The warehouse cannot be deleted yet: its registry catalog is attached to an engine, or a concurrent engine create has just created and assigned its catalog (retry)", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn delete_warehouse(
    State(st): State<CatalogAdminState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, CatalogHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:delete").await?;
    // SP3: `CatalogStore::delete_warehouse_releasing_catalog` owns the
    // transaction that releases the registry catalog's workspace assignments
    // and deletes the warehouse atomically, and 409s (naming the engine(s))
    // instead of a raw FK 500 when an engine still attaches the catalog. The
    // "still its primary" arm went with `engines.warehouse_id` in SP3c-2 Task
    // 6: primacy is now a flag on an attachment, so the attachment check is
    // the whole of it.
    st.store
        .delete_warehouse_releasing_catalog_with_compiler(id, st.perm_compiler.as_deref())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

// Emitted in the merged OpenAPI as `CatalogRoleDto`: the control-plane RBAC
// module already publishes a different-shaped `RoleDto`, and utoipa keys schemas
// by name globally after merge — an unqualified `RoleDto` here would clobber it.
#[derive(Serialize, ToSchema)]
#[schema(as = CatalogRoleDto)]
#[serde(rename_all = "camelCase")]
struct RoleDto {
    id: Uuid,
    name: String,
    consumer_type: String,
    description: Option<String>,
}
impl From<crate::CatalogRole> for RoleDto {
    fn from(r: crate::CatalogRole) -> Self {
        RoleDto {
            id: r.id,
            name: r.name,
            consumer_type: r.consumer_type,
            description: r.description,
        }
    }
}
/// The consumer types the catalog tables accept (their CHECK constraints,
/// migration 0169).
const CONSUMER_TYPES: [&str; 2] = ["engine", "external"];

/// An unknown consumer type is the caller's mistake: 400 naming the accepted
/// values, not a 500 from the CHECK constraint (Task 12, M-8).
fn check_consumer_type(t: &str) -> Result<(), CatalogHttpError> {
    if CONSUMER_TYPES.contains(&t) {
        return Ok(());
    }
    Err(CatalogHttpError::BadRequest(format!(
        "consumerType must be one of {}; got {t:?}",
        CONSUMER_TYPES.join(", ")
    )))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct CreateRoleBody {
    name: String,
    consumer_type: String,
    #[serde(default)]
    description: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/control-plane/warehouses/{id}/roles",
    tag = "control-plane-catalog",
    params(("id" = Uuid, Path, description = "Warehouse id")),
    responses(
        (status = 200, description = "Catalog roles defined on the warehouse", body = Vec<RoleDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_roles(
    State(st): State<CatalogAdminState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<RoleDto>>, CatalogHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:read").await?;
    Ok(Json(
        st.store
            .list_roles(id)
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/warehouses/{id}/roles",
    tag = "control-plane-catalog",
    params(("id" = Uuid, Path, description = "Warehouse id")),
    request_body = CreateRoleBody,
    responses(
        (status = 201, description = "Catalog role created", body = RoleDto),
        (status = 400, description = "Unknown consumerType (engine or external)", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:create permission", body = ErrorBody),
        (status = 409, description = "A role with this name already exists", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn create_role(
    State(st): State<CatalogAdminState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(b): Json<CreateRoleBody>,
) -> Result<(StatusCode, Json<RoleDto>), CatalogHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:create").await?;
    check_consumer_type(&b.consumer_type)?;
    let desc = b.description.as_deref().filter(|d| !d.trim().is_empty());
    let r = st
        .store
        .create_role(id, &b.name, &b.consumer_type, desc)
        .await?;
    Ok((StatusCode::CREATED, Json(r.into())))
}

#[utoipa::path(
    delete,
    path = "/api/control-plane/roles/{role_id}",
    tag = "control-plane-catalog",
    params(("role_id" = Uuid, Path, description = "Catalog role id")),
    responses(
        (status = 204, description = "Catalog role deleted"),
        (status = 400, description = "Role is in use by one or more Catalog Clients", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:delete permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn delete_role(
    State(st): State<CatalogAdminState>,
    Auth(p): Auth,
    Path(role_id): Path<Uuid>,
) -> Result<StatusCode, CatalogHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:delete").await?;
    st.store.delete_role(role_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct GrantDto {
    id: Uuid,
    securable_type: String,
    namespace: Option<Vec<String>>,
    table_name: Option<String>,
    privilege: String,
}
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct AddGrantBody {
    securable_type: String,
    namespace: Option<Vec<String>>,
    table_name: Option<String>,
    privilege: String,
}

#[utoipa::path(
    get,
    path = "/api/control-plane/roles/{role_id}/grants",
    tag = "control-plane-catalog",
    params(("role_id" = Uuid, Path, description = "Catalog role id")),
    responses(
        (status = 200, description = "Grants held by the catalog role", body = Vec<GrantDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_grants(
    State(st): State<CatalogAdminState>,
    Auth(p): Auth,
    Path(role_id): Path<Uuid>,
) -> Result<Json<Vec<GrantDto>>, CatalogHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:read").await?;
    Ok(Json(
        st.store
            .list_grants(role_id)
            .await?
            .into_iter()
            .map(|g| GrantDto {
                id: g.id,
                securable_type: g.securable_type,
                namespace: g.namespace,
                table_name: g.table_name,
                privilege: g.privilege,
            })
            .collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/roles/{role_id}/grants",
    tag = "control-plane-catalog",
    params(("role_id" = Uuid, Path, description = "Catalog role id")),
    request_body = AddGrantBody,
    responses(
        (status = 201, description = "Grant added to the catalog role"),
        (status = 400, description = "Invalid securable/privilege", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:create permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn add_grant(
    State(st): State<CatalogAdminState>,
    Auth(p): Auth,
    Path(role_id): Path<Uuid>,
    Json(b): Json<AddGrantBody>,
) -> Result<StatusCode, CatalogHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:create").await?;
    st.store
        .add_grant(
            role_id,
            &GrantInput {
                securable_type: b.securable_type,
                namespace: b.namespace,
                table_name: b.table_name,
                privilege: b.privilege,
            },
        )
        .await?;
    Ok(StatusCode::CREATED)
}

#[utoipa::path(
    delete,
    path = "/api/control-plane/grants/{grant_id}",
    tag = "control-plane-catalog",
    params(("grant_id" = Uuid, Path, description = "Grant id")),
    responses(
        (status = 204, description = "Grant revoked"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:delete permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn revoke_grant(
    State(st): State<CatalogAdminState>,
    Auth(p): Auth,
    Path(grant_id): Path<Uuid>,
) -> Result<StatusCode, CatalogHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:delete").await?;
    st.store.revoke_grant(grant_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct ClientDto {
    id: Uuid,
    name: String,
    consumer_type: String,
    catalog_role_id: Uuid,
    client_id: String,
}
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct CreatedClientDto {
    id: Uuid,
    client_id: String,
    /// Shown ONCE.
    client_secret: String,
}
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct CreateClientBody {
    name: String,
    consumer_type: String,
    workspace_id: Uuid,
    catalog_role_id: Uuid,
}

#[utoipa::path(
    get,
    path = "/api/control-plane/warehouses/{id}/clients",
    tag = "control-plane-catalog",
    params(("id" = Uuid, Path, description = "Warehouse id")),
    responses(
        (status = 200, description = "Catalog Clients bound to the warehouse", body = Vec<ClientDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_clients(
    State(st): State<CatalogAdminState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<ClientDto>>, CatalogHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:read").await?;
    Ok(Json(
        st.store
            .list_clients(id)
            .await?
            .into_iter()
            .map(|c| ClientDto {
                id: c.id,
                name: c.name,
                consumer_type: c.consumer_type,
                catalog_role_id: c.catalog_role_id,
                client_id: c.client_id,
            })
            .collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/warehouses/{id}/clients",
    tag = "control-plane-catalog",
    params(("id" = Uuid, Path, description = "Warehouse id")),
    request_body = CreateClientBody,
    responses(
        (status = 201, description = "Catalog Client created; client_secret is shown ONCE", body = CreatedClientDto),
        (status = 400, description = "Unknown consumerType (engine or external)", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:create permission", body = ErrorBody),
        (status = 404, description = "No such warehouse or catalog role", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn create_client(
    State(st): State<CatalogAdminState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(b): Json<CreateClientBody>,
) -> Result<(StatusCode, Json<CreatedClientDto>), CatalogHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:create").await?;
    check_consumer_type(&b.consumer_type)?;
    let c = st
        .store
        .create_client(&NewClient {
            name: b.name,
            consumer_type: b.consumer_type,
            workspace_id: b.workspace_id,
            warehouse_id: id,
            catalog_role_id: b.catalog_role_id,
        })
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(CreatedClientDto {
            id: c.id,
            client_id: c.client_id,
            client_secret: c.client_secret,
        }),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/control-plane/clients/{id}",
    tag = "control-plane-catalog",
    params(("id" = Uuid, Path, description = "Catalog Client id")),
    responses(
        (status = 204, description = "Catalog Client revoked"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:delete permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn revoke_client(
    State(st): State<CatalogAdminState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, CatalogHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:delete").await?;
    st.store.revoke_client(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Warehouse browse ─────────────────────────────────────────────────────────

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct NamespaceDto {
    parts: Vec<String>,
    id: Option<Uuid>,
    protected: bool,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct TabularDto {
    name: String,
    kind: String,
    id: Option<Uuid>,
    protected: bool,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct DeletedTabularDto {
    id: Uuid,
    name: String,
    namespace: Vec<String>,
    kind: String,
    deleted_at: chrono::DateTime<chrono::Utc>,
    expiration: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct ColumnDto {
    name: String,
    #[serde(rename = "type")]
    r#type: String,
    required: bool,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct PartitionDto {
    name: String,
    source_column: String,
    transform: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct SortDto {
    column: String,
    transform: String,
    direction: String,
    null_order: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct SnapshotDto {
    snapshot_id: i64,
    parent_snapshot_id: Option<i64>,
    timestamp_ms: i64,
    operation: String,
    summary: std::collections::BTreeMap<String, String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct SchemaDto {
    location: String,
    format_version: u8,
    last_updated_ms: i64,
    current_snapshot_id: Option<i64>,
    snapshot_count: usize,
    columns: Vec<ColumnDto>,
    partition_fields: Vec<PartitionDto>,
    sort_fields: Vec<SortDto>,
    properties: std::collections::BTreeMap<String, String>,
    snapshots: Vec<SnapshotDto>,
}

/// Split a dot-delimited namespace string into its parts.
fn parse_ns(ns: &str) -> Vec<String> {
    ns.split('.').map(|s| s.to_string()).collect()
}

/// Look up the lakekeeper warehouse id for an egret warehouse row.
async fn lk_id(st: &CatalogAdminState, id: Uuid) -> Result<Option<String>, CatalogHttpError> {
    Ok(st
        .store
        .get_warehouse(id)
        .await?
        .and_then(|w| w.lakekeeper_warehouse_id))
}

#[utoipa::path(
    get,
    path = "/api/control-plane/warehouses/{id}/namespaces",
    tag = "control-plane-catalog",
    params(("id" = Uuid, Path, description = "Warehouse id")),
    responses(
        (status = 200, description = "Top-level namespaces (empty when no browser/lakekeeper warehouse is wired)", body = Vec<NamespaceDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:read permission", body = ErrorBody),
        (status = 502, description = "Embedded catalog unreachable", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn browse_namespaces(
    State(st): State<CatalogAdminState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<NamespaceDto>>, CatalogHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:read").await?;
    let (Some(lk), Some(b)) = (lk_id(&st, id).await?, st.browser.as_ref()) else {
        return Ok(Json(vec![]));
    };
    let ns = b.namespaces(&lk).await?;
    Ok(Json(
        ns.into_iter()
            .map(|n| NamespaceDto {
                parts: n.parts,
                id: n.id,
                protected: n.protected,
            })
            .collect(),
    ))
}

#[utoipa::path(
    get,
    path = "/api/control-plane/warehouses/{id}/namespaces/{ns}/tables",
    tag = "control-plane-catalog",
    params(
        ("id" = Uuid, Path, description = "Warehouse id"),
        ("ns" = String, Path, description = "Dot-delimited namespace"),
    ),
    responses(
        (status = 200, description = "Tables and views in the namespace", body = Vec<TabularDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:read permission", body = ErrorBody),
        (status = 502, description = "Embedded catalog unreachable", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn browse_tables(
    State(st): State<CatalogAdminState>,
    Auth(p): Auth,
    Path((id, ns)): Path<(Uuid, String)>,
) -> Result<Json<Vec<TabularDto>>, CatalogHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:read").await?;
    let (Some(lk), Some(b)) = (lk_id(&st, id).await?, st.browser.as_ref()) else {
        return Ok(Json(vec![]));
    };
    let tabulars = b.tabulars(&lk, &parse_ns(&ns)).await?;
    Ok(Json(
        tabulars
            .into_iter()
            .map(|t| TabularDto {
                name: t.name,
                kind: t.kind,
                id: t.id,
                protected: t.protected,
            })
            .collect(),
    ))
}

#[utoipa::path(
    get,
    path = "/api/control-plane/warehouses/{id}/namespaces/{ns}/tables/{table}",
    tag = "control-plane-catalog",
    params(
        ("id" = Uuid, Path, description = "Warehouse id"),
        ("ns" = String, Path, description = "Dot-delimited namespace"),
        ("table" = String, Path, description = "Table name"),
    ),
    responses(
        (status = 200, description = "Table detail (schema, partitioning, snapshots)", body = SchemaDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:read permission", body = ErrorBody),
        (status = 404, description = "No such warehouse/browser or table", body = ErrorBody),
        (status = 502, description = "Embedded catalog unreachable", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn browse_schema(
    State(st): State<CatalogAdminState>,
    Auth(p): Auth,
    Path((id, ns, table)): Path<(Uuid, String, String)>,
) -> Result<Json<SchemaDto>, CatalogHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:read").await?;
    let lk = lk_id(&st, id).await?.ok_or(CatalogHttpError::NotFound)?;
    let b = st.browser.as_ref().ok_or(CatalogHttpError::NotFound)?;
    let d = b.schema(&lk, &parse_ns(&ns), &table).await?;
    Ok(Json(SchemaDto {
        location: d.location,
        format_version: d.format_version,
        last_updated_ms: d.last_updated_ms,
        current_snapshot_id: d.current_snapshot_id,
        snapshot_count: d.snapshot_count,
        columns: d
            .columns
            .into_iter()
            .map(|c| ColumnDto {
                name: c.name,
                r#type: c.type_,
                required: c.required,
            })
            .collect(),
        partition_fields: d
            .partition_fields
            .into_iter()
            .map(|p| PartitionDto {
                name: p.name,
                source_column: p.source_column,
                transform: p.transform,
            })
            .collect(),
        sort_fields: d
            .sort_fields
            .into_iter()
            .map(|s| SortDto {
                column: s.column,
                transform: s.transform,
                direction: s.direction,
                null_order: s.null_order,
            })
            .collect(),
        properties: d.properties,
        snapshots: d
            .snapshots
            .into_iter()
            .map(|s| SnapshotDto {
                snapshot_id: s.snapshot_id,
                parent_snapshot_id: s.parent_snapshot_id,
                timestamp_ms: s.timestamp_ms,
                operation: s.operation,
                summary: s.summary,
            })
            .collect(),
    }))
}

#[utoipa::path(
    get,
    path = "/api/control-plane/warehouses/{id}/namespaces/{ns}/views/{view}",
    tag = "control-plane-catalog",
    params(
        ("id" = Uuid, Path, description = "Warehouse id"),
        ("ns" = String, Path, description = "Dot-delimited namespace"),
        ("view" = String, Path, description = "View name"),
    ),
    responses(
        (status = 200, description = "The view's current-schema columns", body = Vec<ColumnDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:read permission", body = ErrorBody),
        (status = 404, description = "No such warehouse/browser or view", body = ErrorBody),
        (status = 502, description = "Embedded catalog unreachable", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn browse_view(
    State(st): State<CatalogAdminState>,
    Auth(p): Auth,
    Path((id, ns, view)): Path<(Uuid, String, String)>,
) -> Result<Json<Vec<ColumnDto>>, CatalogHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:read").await?;
    let lk = lk_id(&st, id).await?.ok_or(CatalogHttpError::NotFound)?;
    let b = st.browser.as_ref().ok_or(CatalogHttpError::NotFound)?;
    let cols = b.view_columns(&lk, &parse_ns(&ns), &view).await?;
    Ok(Json(
        cols.into_iter()
            .map(|c| ColumnDto {
                name: c.name,
                r#type: c.type_,
                required: c.required,
            })
            .collect(),
    ))
}

#[utoipa::path(
    get,
    path = "/api/control-plane/warehouses/{id}/deleted-tabulars",
    tag = "control-plane-catalog",
    params(("id" = Uuid, Path, description = "Warehouse id")),
    responses(
        (status = 200, description = "Soft-deleted tables/views awaiting purge", body = Vec<DeletedTabularDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:read permission", body = ErrorBody),
        (status = 502, description = "Embedded catalog unreachable", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn browse_deleted(
    State(st): State<CatalogAdminState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<DeletedTabularDto>>, CatalogHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:read").await?;
    let (Some(lk), Some(b)) = (lk_id(&st, id).await?, st.browser.as_ref()) else {
        return Ok(Json(vec![]));
    };
    let items = b.deleted_tabulars(&lk).await?;
    Ok(Json(
        items
            .into_iter()
            .map(|d| DeletedTabularDto {
                id: d.id,
                name: d.name,
                namespace: d.namespace,
                kind: d.kind,
                deleted_at: d.deleted_at,
                expiration: d.expiration,
            })
            .collect(),
    ))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct UndropBody {
    kind: String,
    id: Uuid,
}

#[utoipa::path(
    post,
    path = "/api/control-plane/warehouses/{id}/undrop",
    tag = "control-plane-catalog",
    params(("id" = Uuid, Path, description = "Warehouse id")),
    request_body = UndropBody,
    responses(
        (status = 204, description = "Table/view recovered"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:update permission", body = ErrorBody),
        (status = 404, description = "No such warehouse/browser or tabular", body = ErrorBody),
        (status = 409, description = "A view on a catalog whose profile has classified columns: a restored view would bypass the classification hold, so it is refused; re-create it with CREATE VIEW", body = ErrorBody),
        (status = 502, description = "Embedded catalog unreachable", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn undrop_tabular(
    State(st): State<CatalogAdminState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(b): Json<UndropBody>,
) -> Result<StatusCode, CatalogHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:update").await?;
    let lk = lk_id(&st, id).await?.ok_or(CatalogHttpError::NotFound)?;
    let browser = st.browser.as_ref().ok_or(CatalogHttpError::NotFound)?;
    if b.kind != "view" {
        browser.undrop(&lk, &b.kind, b.id).await?;
        return Ok(StatusCode::NO_CONTENT);
    }
    // View governance B3 (Task 9 fix round 1, I3). An undrop restores a view
    // through lakekeeper's management API, which the view hold never sees: the
    // view would come back with no lineage and no tags, readable over its
    // classified bases. Judging it after the restore leaves it readable
    // untagged until the plugins confirm, and no confirmation can come before
    // a restore egret does not hold. So on a profile with a classified column
    // the undrop is REFUSED (fail-closed); re-creating the view through Trino
    // goes through the hold. The view-tag lock is held across the check and
    // the restore, so a first classification cannot slip in between: it
    // waits, and its own sweep then finds the restored view.
    let db_err = |e: sqlx::Error| {
        tracing::error!(error = %e, "undrop: database");
        CatalogHttpError::Internal
    };
    let mut tx = st.store.pool().begin().await.map_err(db_err)?;
    egret_perm_compile::view_tags::lock_view_tags_in_tx(&mut tx)
        .await
        .map_err(db_err)?;
    if crate::view_hold::profile_has_classified_column(&mut tx, id)
        .await
        .map_err(db_err)?
    {
        return Err(CatalogHttpError::Conflict(
            "egret refused to restore the view: its catalog's profile has classified \
             columns, and a restored view would bypass the classification hold; re-create \
             it with CREATE VIEW from Trino instead"
                .into(),
        ));
    }
    browser.undrop(&lk, &b.kind, b.id).await?;
    tx.commit().await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct ProtectionBody {
    target: String,
    id: Uuid,
    protected: bool,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct ProtectionDto {
    protected: bool,
}

#[utoipa::path(
    put,
    path = "/api/control-plane/warehouses/{id}/protection",
    tag = "control-plane-catalog",
    params(("id" = Uuid, Path, description = "Warehouse id")),
    request_body = ProtectionBody,
    responses(
        (status = 200, description = "New protected flag for the target", body = ProtectionDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:update permission", body = ErrorBody),
        (status = 404, description = "No such warehouse/browser or target", body = ErrorBody),
        (status = 502, description = "Embedded catalog unreachable", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn set_protection(
    State(st): State<CatalogAdminState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(b): Json<ProtectionBody>,
) -> Result<Json<ProtectionDto>, CatalogHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:update").await?;
    let lk = lk_id(&st, id).await?.ok_or(CatalogHttpError::NotFound)?;
    let browser = st.browser.as_ref().ok_or(CatalogHttpError::NotFound)?;
    let protected = browser
        .set_protection(&lk, &b.target, b.id, b.protected)
        .await?;
    Ok(Json(ProtectionDto { protected }))
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct WarehouseConfigDto {
    active: bool,
    delete_profile_seconds: Option<i64>,
    num_tables: i64,
    num_views: i64,
    stats_updated_ms: i64,
}

#[utoipa::path(
    get,
    path = "/api/control-plane/warehouses/{id}/config",
    tag = "control-plane-catalog",
    params(("id" = Uuid, Path, description = "Warehouse id")),
    responses(
        (status = 200, description = "Warehouse admin config (status, soft-delete TTL, statistics)", body = WarehouseConfigDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:read permission", body = ErrorBody),
        (status = 404, description = "No such warehouse/browser", body = ErrorBody),
        (status = 502, description = "Embedded catalog unreachable", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn warehouse_config(
    State(st): State<CatalogAdminState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<WarehouseConfigDto>, CatalogHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:read").await?;
    let lk = lk_id(&st, id).await?.ok_or(CatalogHttpError::NotFound)?;
    let b = st.browser.as_ref().ok_or(CatalogHttpError::NotFound)?;
    let c = b.config(&lk).await?;
    Ok(Json(WarehouseConfigDto {
        active: c.active,
        delete_profile_seconds: c.delete_profile_seconds,
        num_tables: c.num_tables,
        num_views: c.num_views,
        stats_updated_ms: c.stats_updated_ms,
    }))
}

#[derive(Deserialize, ToSchema)]
struct StatusBody {
    active: bool,
}

#[utoipa::path(
    put,
    path = "/api/control-plane/warehouses/{id}/status",
    tag = "control-plane-catalog",
    params(("id" = Uuid, Path, description = "Warehouse id")),
    request_body = StatusBody,
    responses(
        (status = 204, description = "Warehouse activated/deactivated"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:update permission", body = ErrorBody),
        (status = 404, description = "No such warehouse/browser", body = ErrorBody),
        (status = 502, description = "Embedded catalog unreachable", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn set_warehouse_status(
    State(st): State<CatalogAdminState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(b): Json<StatusBody>,
) -> Result<StatusCode, CatalogHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:update").await?;
    let lk = lk_id(&st, id).await?.ok_or(CatalogHttpError::NotFound)?;
    let browser = st.browser.as_ref().ok_or(CatalogHttpError::NotFound)?;
    browser.set_active(&lk, b.active).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, ToSchema)]
struct DeleteProfileBody {
    /// Soft-delete retention in seconds; null = hard delete.
    seconds: Option<i64>,
}

#[utoipa::path(
    put,
    path = "/api/control-plane/warehouses/{id}/delete-profile",
    tag = "control-plane-catalog",
    params(("id" = Uuid, Path, description = "Warehouse id")),
    request_body = DeleteProfileBody,
    responses(
        (status = 204, description = "Soft-delete retention updated"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:update permission", body = ErrorBody),
        (status = 404, description = "No such warehouse/browser", body = ErrorBody),
        (status = 502, description = "Embedded catalog unreachable", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn set_delete_profile_route(
    State(st): State<CatalogAdminState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(b): Json<DeleteProfileBody>,
) -> Result<StatusCode, CatalogHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:update").await?;
    let lk = lk_id(&st, id).await?.ok_or(CatalogHttpError::NotFound)?;
    let browser = st.browser.as_ref().ok_or(CatalogHttpError::NotFound)?;
    browser.set_delete_profile(&lk, b.seconds).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, ToSchema)]
struct FormatPolicyBody {
    versions: Vec<u8>,
}

#[utoipa::path(
    put,
    path = "/api/control-plane/warehouses/{id}/format-policy",
    tag = "control-plane-catalog",
    params(("id" = Uuid, Path, description = "Warehouse id")),
    request_body = FormatPolicyBody,
    responses(
        (status = 204, description = "Allowed Iceberg format versions updated"),
        (status = 400, description = "At least one format version is required", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:update permission", body = ErrorBody),
        (status = 404, description = "No such warehouse/browser", body = ErrorBody),
        (status = 502, description = "Embedded catalog unreachable", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn set_format_policy(
    State(st): State<CatalogAdminState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(b): Json<FormatPolicyBody>,
) -> Result<StatusCode, CatalogHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:update").await?;
    if b.versions.is_empty() {
        return Err(CatalogHttpError::BadRequest(
            "at least one format version required".into(),
        ));
    }
    let lk = lk_id(&st, id).await?.ok_or(CatalogHttpError::NotFound)?;
    let browser = st.browser.as_ref().ok_or(CatalogHttpError::NotFound)?;
    browser.set_format_versions(&lk, b.versions).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct RotateCredBody {
    access_key: String,
    secret_key: String,
}

#[utoipa::path(
    put,
    path = "/api/control-plane/warehouses/{id}/storage-credential",
    tag = "control-plane-catalog",
    params(("id" = Uuid, Path, description = "Warehouse id")),
    request_body = RotateCredBody,
    responses(
        (status = 204, description = "S3 storage credential rotated"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:update permission", body = ErrorBody),
        (status = 404, description = "No such warehouse/browser", body = ErrorBody),
        (status = 502, description = "Embedded catalog unreachable", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn rotate_credential(
    State(st): State<CatalogAdminState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(b): Json<RotateCredBody>,
) -> Result<StatusCode, CatalogHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:update").await?;
    let lk = lk_id(&st, id).await?.ok_or(CatalogHttpError::NotFound)?;
    let browser = st.browser.as_ref().ok_or(CatalogHttpError::NotFound)?;
    browser
        .rotate_credential(&lk, &b.access_key, &b.secret_key)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct SearchResultDto {
    namespace: Vec<String>,
    name: String,
    kind: String,
}

#[derive(Deserialize)]
struct SearchQuery {
    #[serde(default)]
    q: String,
}

#[utoipa::path(
    get,
    path = "/api/control-plane/warehouses/{id}/search",
    tag = "control-plane-catalog",
    params(
        ("id" = Uuid, Path, description = "Warehouse id"),
        ("q" = Option<String>, Query, description = "Table/view name search term (empty ⇒ empty result)"),
    ),
    responses(
        (status = 200, description = "Table/view name-search matches", body = Vec<SearchResultDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:read permission", body = ErrorBody),
        (status = 502, description = "Embedded catalog unreachable", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn search_tabulars(
    State(st): State<CatalogAdminState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    axum::extract::Query(sq): axum::extract::Query<SearchQuery>,
) -> Result<Json<Vec<SearchResultDto>>, CatalogHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:read").await?;
    if sq.q.trim().is_empty() {
        return Ok(Json(vec![]));
    }
    let (Some(lk), Some(b)) = (lk_id(&st, id).await?, st.browser.as_ref()) else {
        return Ok(Json(vec![]));
    };
    let items = b.search(&lk, sq.q.trim()).await?;
    Ok(Json(
        items
            .into_iter()
            .map(|r| SearchResultDto {
                namespace: r.namespace,
                name: r.name,
                kind: r.kind,
            })
            .collect(),
    ))
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct TaskDto {
    queue: String,
    entity: Option<String>,
    status: String,
}

#[utoipa::path(
    get,
    path = "/api/control-plane/warehouses/{id}/tasks",
    tag = "control-plane-catalog",
    params(("id" = Uuid, Path, description = "Warehouse id")),
    responses(
        (status = 200, description = "Recent background (GC) tasks on the warehouse", body = Vec<TaskDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:read permission", body = ErrorBody),
        (status = 502, description = "Embedded catalog unreachable", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_tasks(
    State(st): State<CatalogAdminState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<TaskDto>>, CatalogHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:read").await?;
    let (Some(lk), Some(b)) = (lk_id(&st, id).await?, st.browser.as_ref()) else {
        return Ok(Json(vec![]));
    };
    let items = b.tasks(&lk).await?;
    Ok(Json(
        items
            .into_iter()
            .map(|t| TaskDto {
                queue: t.queue,
                entity: t.entity,
                status: t.status,
            })
            .collect(),
    ))
}

/// OpenAPI fragment for the Catalog seam's warehouse-management admin router
/// (warehouses / catalog-roles / grants / Catalog Clients / browse / recovery /
/// admin ops). Merged into `egret_catalog_engine::openapi()`. `RoleDto` is
/// emitted as `CatalogRoleDto` (see its `#[schema(as = …)]`) so it does not
/// clobber the control-plane RBAC `RoleDto` in the merged doc.
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        list_warehouses,
        create_warehouse,
        get_warehouse,
        delete_warehouse,
        list_roles,
        create_role,
        delete_role,
        list_grants,
        add_grant,
        revoke_grant,
        list_clients,
        create_client,
        revoke_client,
        browse_namespaces,
        browse_tables,
        browse_schema,
        browse_view,
        browse_deleted,
        undrop_tabular,
        set_protection,
        warehouse_config,
        set_warehouse_status,
        set_delete_profile_route,
        set_format_policy,
        rotate_credential,
        search_tabulars,
        list_tasks,
    ),
    components(schemas(
        WarehouseDto,
        CreateWarehouseBody,
        RoleDto,
        CreateRoleBody,
        GrantDto,
        AddGrantBody,
        ClientDto,
        CreatedClientDto,
        CreateClientBody,
        NamespaceDto,
        TabularDto,
        DeletedTabularDto,
        ColumnDto,
        PartitionDto,
        SortDto,
        SnapshotDto,
        SchemaDto,
        UndropBody,
        ProtectionBody,
        ProtectionDto,
        WarehouseConfigDto,
        StatusBody,
        DeleteProfileBody,
        FormatPolicyBody,
        RotateCredBody,
        SearchResultDto,
        TaskDto,
        ErrorBody,
    )),
    tags((name = "control-plane-catalog", description = "Embedded-catalog warehouse management: warehouses, catalog roles/grants/clients, browse, and admin ops")),
)]
pub struct CatalogAdminApi;
