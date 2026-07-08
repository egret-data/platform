//! Control-plane management for the embedded catalog: warehouses, catalog-roles,
//! grants, and Catalog Clients. All routes are platform-admin gated and operate
//! over the egret RBAC store (`CatalogStore`). NOTE: warehouse creation here
//! records the egret-side row; provisioning the lakekeeper warehouse itself is
//! wired with the Authorizer/engine integration (EB-3 T3/T4).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use egret_auth::principal::Auth;
use egret_catalog_engine::{GrantInput, NewClient};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::require_permission;
use crate::error::CpError;
use crate::CpState;

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
    ) -> Result<String, egret_catalog_engine::CatalogEngineError>;
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
    ) -> Result<Vec<BrowseNamespace>, egret_catalog_engine::CatalogEngineError>;
    async fn tabulars(
        &self,
        lakekeeper_warehouse_id: &str,
        namespace: &[String],
    ) -> Result<Vec<BrowseTabular>, egret_catalog_engine::CatalogEngineError>;
    async fn schema(
        &self,
        lakekeeper_warehouse_id: &str,
        namespace: &[String],
        table: &str,
    ) -> Result<BrowseTableDetail, egret_catalog_engine::CatalogEngineError>;
    /// Current-schema columns of a view.
    async fn view_columns(
        &self,
        lakekeeper_warehouse_id: &str,
        namespace: &[String],
        view: &str,
    ) -> Result<Vec<BrowseColumn>, egret_catalog_engine::CatalogEngineError>;
    /// Soft-deleted tables/views awaiting purge.
    async fn deleted_tabulars(
        &self,
        lakekeeper_warehouse_id: &str,
    ) -> Result<Vec<BrowseDeletedTabular>, egret_catalog_engine::CatalogEngineError>;
    /// Recover a soft-deleted table (`kind`="table") or view (`kind`="view").
    async fn undrop(
        &self,
        lakekeeper_warehouse_id: &str,
        kind: &str,
        id: uuid::Uuid,
    ) -> Result<(), egret_catalog_engine::CatalogEngineError>;
    /// Set the protected flag on a `target` ("table"/"view"/"namespace") by id.
    async fn set_protection(
        &self,
        lakekeeper_warehouse_id: &str,
        target: &str,
        id: uuid::Uuid,
        protected: bool,
    ) -> Result<bool, egret_catalog_engine::CatalogEngineError>;
    /// Read the warehouse's admin config (status / soft-delete TTL / statistics).
    async fn config(
        &self,
        lakekeeper_warehouse_id: &str,
    ) -> Result<BrowseWarehouseConfig, egret_catalog_engine::CatalogEngineError>;
    /// Activate/deactivate the warehouse.
    async fn set_active(
        &self,
        lakekeeper_warehouse_id: &str,
        active: bool,
    ) -> Result<(), egret_catalog_engine::CatalogEngineError>;
    /// Set soft-delete retention (`Some(seconds)`) or hard delete (`None`).
    async fn set_delete_profile(
        &self,
        lakekeeper_warehouse_id: &str,
        seconds: Option<i64>,
    ) -> Result<(), egret_catalog_engine::CatalogEngineError>;
    /// Set the allowed Iceberg format versions.
    async fn set_format_versions(
        &self,
        lakekeeper_warehouse_id: &str,
        versions: Vec<u8>,
    ) -> Result<(), egret_catalog_engine::CatalogEngineError>;
    /// Rotate the S3 access-key credential.
    async fn rotate_credential(
        &self,
        lakekeeper_warehouse_id: &str,
        access_key: &str,
        secret_key: &str,
    ) -> Result<(), egret_catalog_engine::CatalogEngineError>;
    /// Search tables/views across the warehouse by name.
    async fn search(
        &self,
        lakekeeper_warehouse_id: &str,
        query: &str,
    ) -> Result<Vec<BrowseSearchResult>, egret_catalog_engine::CatalogEngineError>;
    /// List recent background (GC) tasks on the warehouse.
    async fn tasks(
        &self,
        lakekeeper_warehouse_id: &str,
    ) -> Result<Vec<BrowseTask>, egret_catalog_engine::CatalogEngineError>;
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

pub fn router() -> Router<CpState> {
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WarehouseDto {
    id: Uuid,
    name: String,
    workspace_id: Uuid,
    storage_conn_id: Uuid,
    lakekeeper_warehouse_id: Option<String>,
}
impl From<egret_catalog_engine::Warehouse> for WarehouseDto {
    fn from(w: egret_catalog_engine::Warehouse) -> Self {
        WarehouseDto {
            id: w.id,
            name: w.name,
            workspace_id: w.workspace_id,
            storage_conn_id: w.storage_conn_id,
            lakekeeper_warehouse_id: w.lakekeeper_warehouse_id,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateWarehouseBody {
    name: String,
    workspace_id: Uuid,
    storage_connection_id: Uuid,
}

async fn list_warehouses(
    State(st): State<CpState>,
    Auth(p): Auth,
) -> Result<Json<Vec<WarehouseDto>>, CpError> {
    require_permission(&p, "warehouse:read")?;
    Ok(Json(
        st.storage
            .list_warehouses()
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
    ))
}

async fn create_warehouse(
    State(st): State<CpState>,
    Auth(p): Auth,
    Json(b): Json<CreateWarehouseBody>,
) -> Result<(StatusCode, Json<WarehouseDto>), CpError> {
    require_permission(&p, "warehouse:create")?;
    let w = st
        .storage
        .create_warehouse(&b.name, b.workspace_id, b.storage_connection_id)
        .await?;

    // Provision the actual lakekeeper warehouse (so data ops work). If it fails,
    // roll back the egret row — a warehouse with no lakekeeper counterpart is useless.
    if let Some(prov) = &st.provisioner {
        match provision_lakekeeper(&st, &b.name, b.storage_connection_id, prov.as_ref()).await {
            Ok(lk_id) => st.storage.set_lakekeeper_warehouse_id(w.id, &lk_id).await?,
            Err(e) => {
                let _ = st.storage.delete_warehouse(w.id).await;
                return Err(e);
            }
        }
    }

    let w = st
        .storage
        .get_warehouse(w.id)
        .await?
        .ok_or(CpError::NotFound)?;
    Ok((StatusCode::CREATED, Json(w.into())))
}

/// Resolve the storage connection (+ its secret) and provision the lakekeeper warehouse.
async fn provision_lakekeeper(
    st: &CpState,
    name: &str,
    storage_conn_id: Uuid,
    prov: &dyn WarehouseProvisioner,
) -> Result<String, CpError> {
    let sc = st
        .storage_registry
        .get_storage_conn(storage_conn_id)
        .await?
        .ok_or_else(|| CpError::BadRequest("unknown storage connection".into()))?;
    let secret_key = match sc.secret_ref {
        // Storage-connection secrets are GLOBAL (None scope), per P2.
        Some(ref_id) => st
            .secrets
            .resolve_plaintext(None, ref_id)
            .await
            .map_err(|e| CpError::BadRequest(format!("resolve storage secret: {e}")))?,
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
    prov.provision(name, &s3).await.map_err(CpError::from)
}

async fn get_warehouse(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<WarehouseDto>, CpError> {
    require_permission(&p, "warehouse:read")?;
    let w = st
        .storage
        .get_warehouse(id)
        .await?
        .ok_or(CpError::NotFound)?;
    Ok(Json(w.into()))
}

async fn delete_warehouse(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, CpError> {
    require_permission(&p, "warehouse:delete")?;
    st.storage.delete_warehouse(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RoleDto {
    id: Uuid,
    name: String,
    consumer_type: String,
    description: Option<String>,
}
impl From<egret_catalog_engine::CatalogRole> for RoleDto {
    fn from(r: egret_catalog_engine::CatalogRole) -> Self {
        RoleDto {
            id: r.id,
            name: r.name,
            consumer_type: r.consumer_type,
            description: r.description,
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateRoleBody {
    name: String,
    consumer_type: String,
    #[serde(default)]
    description: Option<String>,
}

async fn list_roles(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<RoleDto>>, CpError> {
    require_permission(&p, "warehouse:read")?;
    Ok(Json(
        st.storage
            .list_roles(id)
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
    ))
}

async fn create_role(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(b): Json<CreateRoleBody>,
) -> Result<(StatusCode, Json<RoleDto>), CpError> {
    require_permission(&p, "warehouse:create")?;
    let desc = b.description.as_deref().filter(|d| !d.trim().is_empty());
    let r = st
        .storage
        .create_role(id, &b.name, &b.consumer_type, desc)
        .await?;
    Ok((StatusCode::CREATED, Json(r.into())))
}

async fn delete_role(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(role_id): Path<Uuid>,
) -> Result<StatusCode, CpError> {
    require_permission(&p, "warehouse:delete")?;
    st.storage.delete_role(role_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GrantDto {
    id: Uuid,
    securable_type: String,
    namespace: Option<Vec<String>>,
    table_name: Option<String>,
    privilege: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddGrantBody {
    securable_type: String,
    namespace: Option<Vec<String>>,
    table_name: Option<String>,
    privilege: String,
}

async fn list_grants(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(role_id): Path<Uuid>,
) -> Result<Json<Vec<GrantDto>>, CpError> {
    require_permission(&p, "warehouse:read")?;
    Ok(Json(
        st.storage
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

async fn add_grant(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(role_id): Path<Uuid>,
    Json(b): Json<AddGrantBody>,
) -> Result<StatusCode, CpError> {
    require_permission(&p, "warehouse:create")?;
    st.storage
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

async fn revoke_grant(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(grant_id): Path<Uuid>,
) -> Result<StatusCode, CpError> {
    require_permission(&p, "warehouse:delete")?;
    st.storage.revoke_grant(grant_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ClientDto {
    id: Uuid,
    name: String,
    consumer_type: String,
    catalog_role_id: Uuid,
    client_id: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreatedClientDto {
    id: Uuid,
    client_id: String,
    /// Shown ONCE.
    client_secret: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateClientBody {
    name: String,
    consumer_type: String,
    workspace_id: Uuid,
    catalog_role_id: Uuid,
}

async fn list_clients(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<ClientDto>>, CpError> {
    require_permission(&p, "warehouse:read")?;
    Ok(Json(
        st.storage
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

async fn create_client(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(b): Json<CreateClientBody>,
) -> Result<(StatusCode, Json<CreatedClientDto>), CpError> {
    require_permission(&p, "warehouse:create")?;
    let c = st
        .storage
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

async fn revoke_client(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, CpError> {
    require_permission(&p, "warehouse:delete")?;
    st.storage.revoke_client(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Warehouse browse ─────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NamespaceDto {
    parts: Vec<String>,
    id: Option<Uuid>,
    protected: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TabularDto {
    name: String,
    kind: String,
    id: Option<Uuid>,
    protected: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeletedTabularDto {
    id: Uuid,
    name: String,
    namespace: Vec<String>,
    kind: String,
    deleted_at: chrono::DateTime<chrono::Utc>,
    expiration: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ColumnDto {
    name: String,
    #[serde(rename = "type")]
    r#type: String,
    required: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PartitionDto {
    name: String,
    source_column: String,
    transform: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SortDto {
    column: String,
    transform: String,
    direction: String,
    null_order: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotDto {
    snapshot_id: i64,
    parent_snapshot_id: Option<i64>,
    timestamp_ms: i64,
    operation: String,
    summary: std::collections::BTreeMap<String, String>,
}

#[derive(Serialize)]
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
async fn lk_id(st: &CpState, id: Uuid) -> Result<Option<String>, CpError> {
    Ok(st
        .storage
        .get_warehouse(id)
        .await?
        .and_then(|w| w.lakekeeper_warehouse_id))
}

async fn browse_namespaces(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<NamespaceDto>>, CpError> {
    require_permission(&p, "warehouse:read")?;
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

async fn browse_tables(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, ns)): Path<(Uuid, String)>,
) -> Result<Json<Vec<TabularDto>>, CpError> {
    require_permission(&p, "warehouse:read")?;
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

async fn browse_schema(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, ns, table)): Path<(Uuid, String, String)>,
) -> Result<Json<SchemaDto>, CpError> {
    require_permission(&p, "warehouse:read")?;
    let lk = lk_id(&st, id).await?.ok_or(CpError::NotFound)?;
    let b = st.browser.as_ref().ok_or(CpError::NotFound)?;
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

async fn browse_view(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, ns, view)): Path<(Uuid, String, String)>,
) -> Result<Json<Vec<ColumnDto>>, CpError> {
    require_permission(&p, "warehouse:read")?;
    let lk = lk_id(&st, id).await?.ok_or(CpError::NotFound)?;
    let b = st.browser.as_ref().ok_or(CpError::NotFound)?;
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

async fn browse_deleted(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<DeletedTabularDto>>, CpError> {
    require_permission(&p, "warehouse:read")?;
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UndropBody {
    kind: String,
    id: Uuid,
}

async fn undrop_tabular(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(b): Json<UndropBody>,
) -> Result<StatusCode, CpError> {
    require_permission(&p, "warehouse:update")?;
    let lk = lk_id(&st, id).await?.ok_or(CpError::NotFound)?;
    let browser = st.browser.as_ref().ok_or(CpError::NotFound)?;
    browser.undrop(&lk, &b.kind, b.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProtectionBody {
    target: String,
    id: Uuid,
    protected: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProtectionDto {
    protected: bool,
}

async fn set_protection(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(b): Json<ProtectionBody>,
) -> Result<Json<ProtectionDto>, CpError> {
    require_permission(&p, "warehouse:update")?;
    let lk = lk_id(&st, id).await?.ok_or(CpError::NotFound)?;
    let browser = st.browser.as_ref().ok_or(CpError::NotFound)?;
    let protected = browser
        .set_protection(&lk, &b.target, b.id, b.protected)
        .await?;
    Ok(Json(ProtectionDto { protected }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WarehouseConfigDto {
    active: bool,
    delete_profile_seconds: Option<i64>,
    num_tables: i64,
    num_views: i64,
    stats_updated_ms: i64,
}

async fn warehouse_config(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<WarehouseConfigDto>, CpError> {
    require_permission(&p, "warehouse:read")?;
    let lk = lk_id(&st, id).await?.ok_or(CpError::NotFound)?;
    let b = st.browser.as_ref().ok_or(CpError::NotFound)?;
    let c = b.config(&lk).await?;
    Ok(Json(WarehouseConfigDto {
        active: c.active,
        delete_profile_seconds: c.delete_profile_seconds,
        num_tables: c.num_tables,
        num_views: c.num_views,
        stats_updated_ms: c.stats_updated_ms,
    }))
}

#[derive(Deserialize)]
struct StatusBody {
    active: bool,
}

async fn set_warehouse_status(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(b): Json<StatusBody>,
) -> Result<StatusCode, CpError> {
    require_permission(&p, "warehouse:update")?;
    let lk = lk_id(&st, id).await?.ok_or(CpError::NotFound)?;
    let browser = st.browser.as_ref().ok_or(CpError::NotFound)?;
    browser.set_active(&lk, b.active).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct DeleteProfileBody {
    /// Soft-delete retention in seconds; null = hard delete.
    seconds: Option<i64>,
}

async fn set_delete_profile_route(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(b): Json<DeleteProfileBody>,
) -> Result<StatusCode, CpError> {
    require_permission(&p, "warehouse:update")?;
    let lk = lk_id(&st, id).await?.ok_or(CpError::NotFound)?;
    let browser = st.browser.as_ref().ok_or(CpError::NotFound)?;
    browser.set_delete_profile(&lk, b.seconds).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct FormatPolicyBody {
    versions: Vec<u8>,
}

async fn set_format_policy(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(b): Json<FormatPolicyBody>,
) -> Result<StatusCode, CpError> {
    require_permission(&p, "warehouse:update")?;
    if b.versions.is_empty() {
        return Err(CpError::BadRequest(
            "at least one format version required".into(),
        ));
    }
    let lk = lk_id(&st, id).await?.ok_or(CpError::NotFound)?;
    let browser = st.browser.as_ref().ok_or(CpError::NotFound)?;
    browser.set_format_versions(&lk, b.versions).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RotateCredBody {
    access_key: String,
    secret_key: String,
}

async fn rotate_credential(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(b): Json<RotateCredBody>,
) -> Result<StatusCode, CpError> {
    require_permission(&p, "warehouse:update")?;
    let lk = lk_id(&st, id).await?.ok_or(CpError::NotFound)?;
    let browser = st.browser.as_ref().ok_or(CpError::NotFound)?;
    browser
        .rotate_credential(&lk, &b.access_key, &b.secret_key)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
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

async fn search_tabulars(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    axum::extract::Query(sq): axum::extract::Query<SearchQuery>,
) -> Result<Json<Vec<SearchResultDto>>, CpError> {
    require_permission(&p, "warehouse:read")?;
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskDto {
    queue: String,
    entity: Option<String>,
    status: String,
}

async fn list_tasks(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<TaskDto>>, CpError> {
    require_permission(&p, "warehouse:read")?;
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
