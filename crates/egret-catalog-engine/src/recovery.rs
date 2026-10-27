//! In-process catalog recovery + protection over lakekeeper's **management API**:
//! list/undrop soft-deleted tabulars and set the `protected` flag on a
//! table/view/namespace. Called on the management `ApiServer` with a system
//! principal (admin sees all), mirroring `browse`/`warehouse`.

use crate::bootstrap::system_request_metadata;
use crate::error::CatalogEngineError;
use crate::mount::EngineAuthorizer;
use crate::mount::EngineState;
use anyhow::Context as _;
use lakekeeper::api::management::v1::namespace::NamespaceManagementService;
use lakekeeper::api::management::v1::table::TableManagementService;
use lakekeeper::api::management::v1::view::ViewManagementService;
use lakekeeper::api::management::v1::warehouse::{
    ListDeletedTabularsQuery, Service as WarehouseMgmtService, UndropTabularsRequest,
};
use lakekeeper::api::management::v1::{ApiServer, TabularType};
use lakekeeper::api::ApiContext;
use lakekeeper::service::{NamespaceId, TableId, TabularId, ViewId, WarehouseId};
use lakekeeper_storage_postgres::{PostgresBackend, SecretsState};
use uuid::Uuid;

type EngineApiServer = ApiServer<PostgresBackend, EngineAuthorizer, SecretsState>;

/// The kind of a tabular (only tables + views are surfaced; generic tables are skipped).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TabularKindName {
    Table,
    View,
}

/// A soft-deleted table or view still recoverable before the GC purge.
pub struct DeletedTabular {
    pub id: Uuid,
    pub name: String,
    pub namespace: Vec<String>,
    pub kind: TabularKindName,
    pub deleted_at: chrono::DateTime<chrono::Utc>,
    pub expiration: chrono::DateTime<chrono::Utc>,
}

/// What a protection toggle targets.
#[derive(Clone, Copy)]
pub enum ProtectTarget {
    Table,
    View,
    Namespace,
}

fn wh(warehouse_id: &str) -> Result<WarehouseId, CatalogEngineError> {
    let uuid: Uuid = warehouse_id.parse().map_err(|_| {
        CatalogEngineError::BadRequest("lakekeeper warehouse id is not a UUID".into())
    })?;
    Ok(WarehouseId::from(uuid))
}

/// List soft-deleted tables/views (first page) awaiting purge.
pub async fn list_deleted_tabulars(
    engine_state: ApiContext<EngineState>,
    system_user: &str,
    warehouse_id: &str,
) -> Result<Vec<DeletedTabular>, CatalogEngineError> {
    let rm = system_request_metadata(system_user).context("system metadata")?;
    let resp = <EngineApiServer as WarehouseMgmtService<
        PostgresBackend,
        EngineAuthorizer,
        SecretsState,
    >>::list_soft_deleted_tabulars(
        wh(warehouse_id)?,
        ListDeletedTabularsQuery {
            namespace_id: None,
            page_token: None,
            page_size: None,
        },
        engine_state,
        rm,
    )
    .await
    .map_err(|e| CatalogEngineError::from_lakekeeper("list_soft_deleted_tabulars", e))?;

    Ok(resp
        .tabulars
        .iter()
        .filter_map(|t| {
            let kind = match t.typ {
                TabularType::Table => TabularKindName::Table,
                TabularType::View => TabularKindName::View,
                TabularType::GenericTable => return None,
            };
            Some(DeletedTabular {
                id: t.id,
                name: t.name.clone(),
                namespace: t.namespace.clone(),
                kind,
                deleted_at: t.deleted_at,
                expiration: t.expiration_date,
            })
        })
        .collect())
}

/// Recover a soft-deleted table or view.
pub async fn undrop_tabular(
    engine_state: ApiContext<EngineState>,
    system_user: &str,
    warehouse_id: &str,
    kind: TabularKindName,
    id: Uuid,
) -> Result<(), CatalogEngineError> {
    let rm = system_request_metadata(system_user).context("system metadata")?;
    let target = match kind {
        TabularKindName::Table => TabularId::Table(TableId::from(id)),
        TabularKindName::View => TabularId::View(ViewId::from(id)),
    };
    <EngineApiServer as WarehouseMgmtService<PostgresBackend, EngineAuthorizer, SecretsState>>::undrop_tabulars(
        wh(warehouse_id)?,
        rm,
        UndropTabularsRequest { targets: vec![target] },
        engine_state,
    )
    .await
    .map_err(|e| CatalogEngineError::from_lakekeeper("undrop_tabulars", e))?;
    Ok(())
}

/// Set (or clear) the `protected` flag on a table/view/namespace. Returns the new state.
pub async fn set_protection(
    engine_state: ApiContext<EngineState>,
    system_user: &str,
    warehouse_id: &str,
    target: ProtectTarget,
    id: Uuid,
    protected: bool,
) -> Result<bool, CatalogEngineError> {
    let rm = system_request_metadata(system_user).context("system metadata")?;
    let wid = wh(warehouse_id)?;
    let resp = match target {
        ProtectTarget::Table => {
            <EngineApiServer as TableManagementService<
                PostgresBackend,
                EngineAuthorizer,
                SecretsState,
            >>::set_table_protection(TableId::from(id), wid, protected, engine_state, rm)
            .await
        }
        ProtectTarget::View => {
            <EngineApiServer as ViewManagementService<
                PostgresBackend,
                EngineAuthorizer,
                SecretsState,
            >>::set_view_protection(ViewId::from(id), wid, protected, engine_state, rm)
            .await
        }
        ProtectTarget::Namespace => {
            <EngineApiServer as NamespaceManagementService<
                PostgresBackend,
                EngineAuthorizer,
                SecretsState,
            >>::set_namespace_protection(
                NamespaceId::from(id), wid, protected, engine_state, rm
            )
            .await
        }
    }
    .map_err(|e| CatalogEngineError::from_lakekeeper("set_protection", e))?;
    Ok(resp.protected)
}
