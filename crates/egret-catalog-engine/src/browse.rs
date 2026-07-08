//! In-process catalog browse: list namespaces/tables and load a table's schema
//! from the embedded lakekeeper, using a system principal (admin sees all).
//!
//! Adapted from the read helpers of the pre-EB-1 `browse.rs` (PR #90); only the
//! lakekeeper service calls are kept — no routing, workspace-scoping, or DDL.

use crate::bootstrap::system_request_metadata;
use crate::error::CatalogEngineError;
use crate::mount::{EngineServer, EngineState};
use anyhow::Context as _;
use lakekeeper::api::iceberg::types::{PageToken, Prefix};
use lakekeeper::api::iceberg::v1::namespace::{
    ListNamespacesQuery, NamespaceParameters, NamespaceService,
};
use lakekeeper::api::iceberg::v1::tables::{
    ListTablesQuery, LoadTableRequest, LoadTableResultOrNotModified, TableParameters, TablesService,
};
use lakekeeper::api::iceberg::v1::views::{LoadViewRequest, ViewParameters, ViewService};
use lakekeeper::api::ApiContext;
use lakekeeper::iceberg::{NamespaceIdent, TableIdent};
use uuid::Uuid;

/// A namespace as its multipart identifier (Iceberg namespaces can be nested).
/// `id` is the lakekeeper namespace UUID (for protection); `protected` blocks drops.
pub struct NamespaceEntry {
    pub parts: Vec<String>,
    pub id: Option<Uuid>,
    pub protected: bool,
}

pub enum TabularKind {
    Table,
    View,
}

pub struct TabularEntry {
    pub name: String,
    pub kind: TabularKind,
    pub id: Option<Uuid>,
    pub protected: bool,
}

pub struct ColumnInfo {
    pub name: String,
    pub r#type: String,
    pub required: bool,
}

pub struct PartitionField {
    pub name: String,
    pub source_column: String,
    pub transform: String,
}

pub struct SortField {
    pub column: String,
    pub transform: String,
    pub direction: String,
    pub null_order: String,
}

pub struct SnapshotInfo {
    pub snapshot_id: i64,
    pub parent_snapshot_id: Option<i64>,
    pub timestamp_ms: i64,
    pub operation: String,
    pub summary: std::collections::BTreeMap<String, String>,
}

/// A UI-friendly projection of a table's current metadata (no query execution).
pub struct TableDetail {
    pub location: String,
    pub format_version: u8,
    pub last_updated_ms: i64,
    pub current_snapshot_id: Option<i64>,
    pub snapshot_count: usize,
    pub columns: Vec<ColumnInfo>,
    pub partition_fields: Vec<PartitionField>,
    pub sort_fields: Vec<SortField>,
    pub properties: std::collections::BTreeMap<String, String>,
    pub snapshots: Vec<SnapshotInfo>,
}

fn prefix(warehouse_id: &str) -> Prefix {
    Prefix::from(warehouse_id.to_string())
}

/// BFS the whole namespace tree (lakekeeper returns one level per call), flattening
/// each namespace to its dotted levels.
pub async fn list_namespaces(
    engine_state: ApiContext<EngineState>,
    system_user: &str,
    warehouse_id: &str,
) -> Result<Vec<NamespaceEntry>, CatalogEngineError> {
    let px = prefix(warehouse_id);
    let mut out: Vec<NamespaceEntry> = Vec::new();
    let mut frontier: Vec<Option<NamespaceIdent>> = vec![None];
    while let Some(parent) = frontier.pop() {
        let mut page = PageToken::Empty;
        loop {
            let query = ListNamespacesQuery {
                page_token: page.clone(),
                page_size: None,
                parent: parent.clone(),
                return_uuids: true,
                return_protection_status: true,
            };
            let rm = system_request_metadata(system_user).context("system metadata")?;
            let resp = <EngineServer as NamespaceService<EngineState>>::list_namespaces(
                Some(px.clone()),
                query,
                engine_state.clone(),
                rm,
            )
            .await
            .map_err(|e| CatalogEngineError::from_lakekeeper("list_namespaces", e))?;
            let uuids = resp.namespace_uuids.clone().unwrap_or_default();
            let protection = resp.protection_status.clone().unwrap_or_default();
            for (i, ns) in resp.namespaces.iter().enumerate() {
                let levels = ns.clone().inner();
                frontier.push(Some(
                    NamespaceIdent::from_vec(levels.clone())
                        .map_err(|e| anyhow::anyhow!("namespace ident: {e:?}"))?,
                ));
                out.push(NamespaceEntry {
                    parts: levels,
                    id: uuids.get(i).copied(),
                    protected: protection.get(i).copied().unwrap_or(false),
                });
            }
            match resp.next_page_token {
                Some(t) if !t.is_empty() => page = PageToken::Present(t),
                _ => break,
            }
        }
    }
    out.sort_by(|a, b| a.parts.cmp(&b.parts));
    Ok(out)
}

/// List the tables and views in a namespace.
pub async fn list_tabulars(
    engine_state: ApiContext<EngineState>,
    system_user: &str,
    warehouse_id: &str,
    namespace: &[String],
) -> Result<Vec<TabularEntry>, CatalogEngineError> {
    let params = NamespaceParameters {
        prefix: Some(prefix(warehouse_id)),
        namespace: NamespaceIdent::from_vec(namespace.to_vec())
            .map_err(|e| CatalogEngineError::BadRequest(format!("namespace ident: {e:?}")))?,
    };

    let mut out: Vec<TabularEntry> = Vec::new();
    // Tables.
    let mut page = PageToken::Empty;
    loop {
        let query = list_query(page.clone());
        let rm = system_request_metadata(system_user).context("system metadata")?;
        let resp = <EngineServer as TablesService<EngineState>>::list_tables(
            params.clone(),
            query,
            engine_state.clone(),
            rm,
        )
        .await
        .map_err(|e| CatalogEngineError::from_lakekeeper("list_tables", e))?;
        let uuids = resp.table_uuids.clone().unwrap_or_default();
        let protection = resp.protection_status.clone().unwrap_or_default();
        for (i, ident) in resp.identifiers.iter().enumerate() {
            out.push(TabularEntry {
                name: ident.name.clone(),
                kind: TabularKind::Table,
                id: uuids.get(i).copied(),
                protected: protection.get(i).copied().unwrap_or(false),
            });
        }
        match resp.next_page_token {
            Some(t) if !t.is_empty() => page = PageToken::Present(t),
            _ => break,
        }
    }
    // Views.
    let mut page = PageToken::Empty;
    loop {
        let query = list_query(page.clone());
        let rm = system_request_metadata(system_user).context("system metadata")?;
        let resp = <EngineServer as ViewService<EngineState>>::list_views(
            params.clone(),
            query,
            engine_state.clone(),
            rm,
        )
        .await
        .map_err(|e| CatalogEngineError::from_lakekeeper("list_views", e))?;
        let uuids = resp.table_uuids.clone().unwrap_or_default();
        let protection = resp.protection_status.clone().unwrap_or_default();
        for (i, ident) in resp.identifiers.iter().enumerate() {
            out.push(TabularEntry {
                name: ident.name.clone(),
                kind: TabularKind::View,
                id: uuids.get(i).copied(),
                protected: protection.get(i).copied().unwrap_or(false),
            });
        }
        match resp.next_page_token {
            Some(t) if !t.is_empty() => page = PageToken::Present(t),
            _ => break,
        }
    }
    Ok(out)
}

fn list_query(page_token: PageToken) -> ListTablesQuery {
    ListTablesQuery {
        page_token,
        page_size: None,
        return_uuids: true,
        return_protection_status: true,
    }
}

/// Load a view's current-schema columns (views have no snapshots/partitions).
pub async fn load_view_detail(
    engine_state: ApiContext<EngineState>,
    system_user: &str,
    warehouse_id: &str,
    namespace: &[String],
    view: &str,
) -> Result<Vec<ColumnInfo>, CatalogEngineError> {
    let ident = TableIdent {
        namespace: NamespaceIdent::from_vec(namespace.to_vec())
            .map_err(|e| CatalogEngineError::BadRequest(format!("namespace ident: {e:?}")))?,
        name: view.to_string(),
    };
    let rm = system_request_metadata(system_user).context("system metadata")?;
    let loaded = <EngineServer as ViewService<EngineState>>::load_view(
        ViewParameters {
            prefix: Some(prefix(warehouse_id)),
            view: ident,
        },
        LoadViewRequest::default(),
        engine_state,
        rm,
    )
    .await
    .map_err(|e| CatalogEngineError::from_lakekeeper("load_view", e))?;

    let schema = loaded.metadata.current_schema();
    Ok(schema
        .as_struct()
        .fields()
        .iter()
        .map(|f| ColumnInfo {
            name: f.name.clone(),
            r#type: f.field_type.to_string(),
            required: f.required,
        })
        .collect())
}

/// Load a UI-friendly projection of a table's current metadata: schema columns,
/// partition/sort fields (with transforms), location, format version, properties,
/// and snapshot history — all from the metadata the load already returns.
pub async fn load_table_detail(
    engine_state: ApiContext<EngineState>,
    system_user: &str,
    warehouse_id: &str,
    namespace: &[String],
    table: &str,
) -> Result<TableDetail, CatalogEngineError> {
    let ident = TableIdent {
        namespace: NamespaceIdent::from_vec(namespace.to_vec())
            .map_err(|e| CatalogEngineError::BadRequest(format!("namespace ident: {e:?}")))?,
        name: table.to_string(),
    };
    let rm = system_request_metadata(system_user).context("system metadata")?;
    let loaded = <EngineServer as TablesService<EngineState>>::load_table(
        TableParameters {
            prefix: Some(prefix(warehouse_id)),
            table: ident,
        },
        LoadTableRequest::default(),
        engine_state,
        rm,
    )
    .await
    .map_err(|e| CatalogEngineError::from_lakekeeper("load_table", e))?;

    let result = match loaded {
        LoadTableResultOrNotModified::LoadTableResult(res) => res,
        // We send no if-none-match etags, so the engine never returns 304.
        LoadTableResultOrNotModified::NotModifiedResponse(_) => {
            return Err(CatalogEngineError::Internal(
                "unexpected 304 from load_table".into(),
            ))
        }
    };

    let md = &result.metadata;
    let schema = md.current_schema();
    let columns = schema
        .as_struct()
        .fields()
        .iter()
        .map(|f| ColumnInfo {
            name: f.name.clone(),
            r#type: f.field_type.to_string(),
            required: f.required,
        })
        .collect();

    // Resolve a partition/sort source column id to its schema column name.
    let col_name = |id: i32| {
        schema
            .field_by_id(id)
            .map(|f| f.name.clone())
            .unwrap_or_else(|| id.to_string())
    };
    let partition_fields = md
        .default_partition_spec()
        .fields()
        .iter()
        .map(|pf| PartitionField {
            name: pf.name.clone(),
            source_column: col_name(pf.source_id),
            transform: pf.transform.to_string(),
        })
        .collect();
    let sort_fields = md
        .default_sort_order()
        .fields
        .iter()
        .map(|sf| SortField {
            column: col_name(sf.source_id),
            transform: sf.transform.to_string(),
            direction: sf.direction.to_string(),
            null_order: sf.null_order.to_string(),
        })
        .collect();

    let properties = md
        .properties()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    let mut snapshots: Vec<SnapshotInfo> = md
        .snapshots()
        .map(|s| SnapshotInfo {
            snapshot_id: s.snapshot_id(),
            parent_snapshot_id: s.parent_snapshot_id(),
            timestamp_ms: s.timestamp_ms(),
            operation: s.summary().operation.as_str().to_string(),
            summary: s
                .summary()
                .additional_properties
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        })
        .collect();
    snapshots.sort_by_key(|s| std::cmp::Reverse(s.timestamp_ms));

    Ok(TableDetail {
        location: md.location().to_string(),
        format_version: md.format_version() as u8,
        last_updated_ms: md.last_updated_ms(),
        current_snapshot_id: md.current_snapshot_id(),
        snapshot_count: md.snapshots().count(),
        columns,
        partition_fields,
        sort_fields,
        properties,
        snapshots,
    })
}
