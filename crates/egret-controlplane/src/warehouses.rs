//! Control-plane management for the embedded catalog: warehouses, catalog-roles,
//! grants, and Catalog Clients. All routes are platform-admin gated and operate
//! over the egret RBAC store (`CatalogStore`). NOTE: warehouse creation here
//! records the egret-side row; provisioning the lakekeeper warehouse itself is
//! wired with the Authorizer/engine integration (EB-3 T3/T4).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use axum::{Json, Router};
use egret_auth::principal::Auth;
use egret_catalog::service::embedded::{GrantInput, NewClient};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::require_platform_admin;
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
    async fn provision(&self, name: &str, s3: &ProvisionS3) -> Result<String, String>;
}

/// Catalog browse result rows (control-plane DTOs; camelCase on the wire).
pub struct BrowseNamespace {
    pub parts: Vec<String>,
}
pub struct BrowseTabular {
    pub name: String,
    pub kind: String, // "table" | "view"
}
pub struct BrowseColumn {
    pub name: String,
    pub type_: String,
    pub required: bool,
}
pub struct BrowseSchema {
    pub columns: Vec<BrowseColumn>,
    pub partition_by: Vec<String>,
    pub sort_by: Vec<String>,
}

/// Reads catalog structure (namespaces/tables/schema) from the embedded
/// lakekeeper. Implemented in egret-bin over the lakekeeper `ApiContext`
/// (like `WarehouseProvisioner`). All args key by the lakekeeper warehouse id.
#[async_trait::async_trait]
pub trait WarehouseBrowser: Send + Sync {
    async fn namespaces(&self, lakekeeper_warehouse_id: &str) -> Result<Vec<BrowseNamespace>, String>;
    async fn tabulars(
        &self,
        lakekeeper_warehouse_id: &str,
        namespace: &[String],
    ) -> Result<Vec<BrowseTabular>, String>;
    async fn schema(
        &self,
        lakekeeper_warehouse_id: &str,
        namespace: &[String],
        table: &str,
    ) -> Result<BrowseSchema, String>;
}

pub fn router() -> Router<CpState> {
    Router::new()
        .route("/warehouses", get(list_warehouses).post(create_warehouse))
        .route(
            "/warehouses/{id}",
            get(get_warehouse).delete(delete_warehouse),
        )
        .route("/warehouses/{id}/roles", get(list_roles).post(create_role))
        .route(
            "/warehouses/{id}/clients",
            get(list_clients).post(create_client),
        )
        .route("/roles/{role_id}/grants", get(list_grants).post(add_grant))
        .route("/grants/{grant_id}", delete(revoke_grant))
        .route("/clients/{id}", delete(revoke_client))
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
impl From<egret_catalog::service::Warehouse> for WarehouseDto {
    fn from(w: egret_catalog::service::Warehouse) -> Self {
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
    require_platform_admin(&p)?;
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
    require_platform_admin(&p)?;
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
        .storage
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
    prov.provision(name, &s3)
        .await
        .map_err(|e| CpError::BadRequest(format!("warehouse provisioning failed: {e}")))
}

async fn get_warehouse(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<WarehouseDto>, CpError> {
    require_platform_admin(&p)?;
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
    require_platform_admin(&p)?;
    st.storage.delete_warehouse(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RoleDto {
    id: Uuid,
    name: String,
    consumer_type: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateRoleBody {
    name: String,
    consumer_type: String,
}

async fn list_roles(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<RoleDto>>, CpError> {
    require_platform_admin(&p)?;
    Ok(Json(
        st.storage
            .list_roles(id)
            .await?
            .into_iter()
            .map(|r| RoleDto {
                id: r.id,
                name: r.name,
                consumer_type: r.consumer_type,
            })
            .collect(),
    ))
}

async fn create_role(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(b): Json<CreateRoleBody>,
) -> Result<(StatusCode, Json<RoleDto>), CpError> {
    require_platform_admin(&p)?;
    let r = st
        .storage
        .create_role(id, &b.name, &b.consumer_type)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(RoleDto {
            id: r.id,
            name: r.name,
            consumer_type: r.consumer_type,
        }),
    ))
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
    require_platform_admin(&p)?;
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
    require_platform_admin(&p)?;
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
    require_platform_admin(&p)?;
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
    require_platform_admin(&p)?;
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
    require_platform_admin(&p)?;
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
    require_platform_admin(&p)?;
    st.storage.revoke_client(id).await?;
    Ok(StatusCode::NO_CONTENT)
}
