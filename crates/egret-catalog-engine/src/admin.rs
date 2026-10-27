//! In-process warehouse administration over lakekeeper's **management API**:
//! read config (status / soft-delete TTL / statistics), activate/deactivate,
//! update the soft-delete profile + format-version policy, and rotate the S3
//! storage credential. System principal, like `browse`/`recovery`.

use crate::bootstrap::system_request_metadata;
use crate::error::CatalogEngineError;
use crate::mount::EngineAuthorizer;
use crate::mount::EngineState;
use anyhow::Context as _;
use lakekeeper::api::iceberg::types::PageToken;
use lakekeeper::api::management::v1::warehouse::{
    Service as WarehouseMgmtService, TabularDeleteProfile, UpdateWarehouseCredentialRequest,
    UpdateWarehouseDeleteProfileRequest, UpdateWarehouseFormatVersionPolicyRequest,
    WarehouseStatus,
};
use lakekeeper::api::management::v1::{ApiServer, GetWarehouseStatisticsQuery};
use lakekeeper::api::ApiContext;
use lakekeeper::service::storage::StorageCredential;
use lakekeeper::service::WarehouseId;
use lakekeeper_storage_postgres::{PostgresBackend, SecretsState};
use uuid::Uuid;

type EngineApiServer = ApiServer<PostgresBackend, EngineAuthorizer, SecretsState>;

/// A UI-friendly projection of a warehouse's admin config.
pub struct WarehouseConfig {
    pub active: bool,
    /// Soft-delete retention in seconds; `None` = hard delete (no recovery).
    pub delete_profile_seconds: Option<i64>,
    pub num_tables: i64,
    pub num_views: i64,
    /// Epoch-millis of the latest statistics sample (0 if none yet).
    pub stats_updated_ms: i64,
}

fn wh(warehouse_id: &str) -> Result<WarehouseId, CatalogEngineError> {
    let uuid: Uuid = warehouse_id.parse().map_err(|_| {
        CatalogEngineError::BadRequest("lakekeeper warehouse id is not a UUID".into())
    })?;
    Ok(WarehouseId::from(uuid))
}

/// Read status + soft-delete TTL + the most-recent table/view counts.
pub async fn get_warehouse_config(
    engine_state: ApiContext<EngineState>,
    system_user: &str,
    warehouse_id: &str,
) -> Result<WarehouseConfig, CatalogEngineError> {
    let wid = wh(warehouse_id)?;
    let rm = system_request_metadata(system_user).context("system metadata")?;
    let info = <EngineApiServer as WarehouseMgmtService<_, _, _>>::get_warehouse(
        wid,
        engine_state.clone(),
        rm,
    )
    .await
    .map_err(|e| CatalogEngineError::from_lakekeeper("get_warehouse", e))?;

    let rm = system_request_metadata(system_user).context("system metadata")?;
    let stats = <EngineApiServer as WarehouseMgmtService<_, _, _>>::get_warehouse_statistics(
        wid,
        GetWarehouseStatisticsQuery {
            page_token: PageToken::Empty,
            page_size: None,
        },
        engine_state,
        rm,
    )
    .await
    .map_err(|e| CatalogEngineError::from_lakekeeper("get_warehouse_statistics", e))?;
    let latest = stats.stats.first();

    Ok(WarehouseConfig {
        active: matches!(info.status, WarehouseStatus::Active),
        delete_profile_seconds: match info.delete_profile {
            TabularDeleteProfile::Soft { expiration_seconds } => {
                Some(expiration_seconds.num_seconds())
            }
            TabularDeleteProfile::Hard {} => None,
        },
        num_tables: latest.map(|s| s.number_of_tables).unwrap_or(0),
        num_views: latest.map(|s| s.number_of_views).unwrap_or(0),
        stats_updated_ms: latest.map(|s| s.timestamp.timestamp_millis()).unwrap_or(0),
    })
}

/// Activate or deactivate a warehouse (an inactive warehouse rejects data ops).
pub async fn set_warehouse_active(
    engine_state: ApiContext<EngineState>,
    system_user: &str,
    warehouse_id: &str,
    active: bool,
) -> Result<(), CatalogEngineError> {
    let wid = wh(warehouse_id)?;
    let rm = system_request_metadata(system_user).context("system metadata")?;
    if active {
        <EngineApiServer as WarehouseMgmtService<_, _, _>>::activate_warehouse(
            wid,
            engine_state,
            rm,
        )
        .await
        .map_err(|e| CatalogEngineError::from_lakekeeper("activate_warehouse", e))?;
    } else {
        <EngineApiServer as WarehouseMgmtService<_, _, _>>::deactivate_warehouse(
            wid,
            engine_state,
            rm,
        )
        .await
        .map_err(|e| CatalogEngineError::from_lakekeeper("deactivate_warehouse", e))?;
    }
    Ok(())
}

/// Set the soft-delete retention (`Some(seconds)`) or switch to hard delete (`None`).
pub async fn set_delete_profile(
    engine_state: ApiContext<EngineState>,
    system_user: &str,
    warehouse_id: &str,
    seconds: Option<i64>,
) -> Result<(), CatalogEngineError> {
    let delete_profile = match seconds {
        Some(s) => TabularDeleteProfile::Soft {
            expiration_seconds: chrono::Duration::seconds(s),
        },
        None => TabularDeleteProfile::Hard {},
    };
    let rm = system_request_metadata(system_user).context("system metadata")?;
    <EngineApiServer as WarehouseMgmtService<_, _, _>>::update_warehouse_delete_profile(
        wh(warehouse_id)?,
        UpdateWarehouseDeleteProfileRequest { delete_profile },
        engine_state,
        rm,
    )
    .await
    .map_err(|e| CatalogEngineError::from_lakekeeper("update_warehouse_delete_profile", e))?;
    Ok(())
}

/// Set the allowed Iceberg format versions (e.g. `[2]` to require v2+).
pub async fn set_format_versions(
    engine_state: ApiContext<EngineState>,
    system_user: &str,
    warehouse_id: &str,
    versions: &[u8],
) -> Result<(), CatalogEngineError> {
    // Build the whole request via serde (ints -> FormatVersion) to avoid coupling
    // to the enum variants; default to the highest allowed version.
    let default = versions.iter().copied().max().unwrap_or(2);
    let request: UpdateWarehouseFormatVersionPolicyRequest =
        serde_json::from_value(serde_json::json!({
            "allowed-format-versions": versions,
            "default-format-version": default,
        }))
        .context("invalid format version policy")?;
    let rm = system_request_metadata(system_user).context("system metadata")?;
    <EngineApiServer as WarehouseMgmtService<_, _, _>>::update_warehouse_format_version_policy(
        wh(warehouse_id)?,
        request,
        engine_state,
        rm,
    )
    .await
    .map_err(|e| {
        CatalogEngineError::from_lakekeeper("update_warehouse_format_version_policy", e)
    })?;
    Ok(())
}

/// Rotate the warehouse's S3 access-key credential (no re-provision).
pub async fn rotate_storage_credential(
    engine_state: ApiContext<EngineState>,
    system_user: &str,
    warehouse_id: &str,
    access_key: &str,
    secret_key: &str,
) -> Result<(), CatalogEngineError> {
    let cred: StorageCredential = serde_json::from_value(serde_json::json!({
        "type": "s3",
        "credential-type": "access-key",
        "access-key-id": access_key,
        "secret-access-key": secret_key,
    }))
    .context("building S3 storage credential")?;
    let rm = system_request_metadata(system_user).context("system metadata")?;
    <EngineApiServer as WarehouseMgmtService<_, _, _>>::update_storage_credential(
        wh(warehouse_id)?,
        UpdateWarehouseCredentialRequest {
            new_storage_credential: Some(cred),
        },
        engine_state,
        rm,
    )
    .await
    .map_err(|e| CatalogEngineError::from_lakekeeper("update_storage_credential", e))?;
    Ok(())
}

/// One tabular match from a warehouse-wide name search.
pub struct SearchResult {
    pub namespace: Vec<String>,
    pub name: String,
    pub kind: &'static str, // "table" | "view"
}

/// Search tables/views across the whole warehouse by (fuzzy) name.
pub async fn search_tabulars(
    engine_state: ApiContext<EngineState>,
    system_user: &str,
    warehouse_id: &str,
    query: &str,
) -> Result<Vec<SearchResult>, CatalogEngineError> {
    use lakekeeper::api::management::v1::tabular::{
        SearchTabularRequest, TabularManagementService,
    };
    use lakekeeper::service::TabularId;
    let rm = system_request_metadata(system_user).context("system metadata")?;
    let resp = <EngineApiServer as TabularManagementService<_, _, _>>::search_tabular(
        wh(warehouse_id)?,
        engine_state,
        rm,
        SearchTabularRequest {
            search: query.to_string(),
        },
    )
    .await
    .map_err(|e| CatalogEngineError::from_lakekeeper("search_tabular", e))?;
    Ok(resp
        .tabulars
        .into_iter()
        .map(|t| SearchResult {
            namespace: t.namespace_name,
            name: t.tabular_name,
            kind: match t.tabular_id {
                TabularId::View(_) => "view",
                _ => "table",
            },
        })
        .collect())
}

/// One background (GC) task on the warehouse.
pub struct TaskInfo {
    pub queue: String,
    pub entity: Option<String>,
    pub status: String,
}

/// List the warehouse's recent background tasks (tabular expiry / purge / task-log).
pub async fn list_tasks(
    engine_state: ApiContext<EngineState>,
    system_user: &str,
    warehouse_id: &str,
) -> Result<Vec<TaskInfo>, CatalogEngineError> {
    use lakekeeper::api::management::v1::tasks::{ListTasksRequest, Service as TasksMgmtService};
    let request: ListTasksRequest =
        serde_json::from_value(serde_json::json!({})).context("list tasks request")?;
    let rm = system_request_metadata(system_user).context("system metadata")?;
    let resp = <EngineApiServer as TasksMgmtService<_, _, _>>::list_tasks(
        wh(warehouse_id)?,
        request,
        engine_state,
        rm,
    )
    .await
    .map_err(|e| CatalogEngineError::from_lakekeeper("list_tasks", e))?;
    Ok(resp
        .tasks
        .into_iter()
        .map(|t| TaskInfo {
            queue: serde_json::to_value(&t.queue_name)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| format!("{:?}", t.queue_name)),
            entity: t.entity_name.map(|n| n.join(".")),
            status: serde_json::to_value(t.status)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| format!("{:?}", t.status)),
        })
        .collect())
}
