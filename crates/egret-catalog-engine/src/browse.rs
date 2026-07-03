//! In-process catalog browse: list namespaces/tables and load a table's schema
//! from the embedded lakekeeper, using a system principal (admin sees all).
//!
//! Adapted from the read helpers of the pre-EB-1 `browse.rs` (PR #90); only the
//! lakekeeper service calls are kept — no routing, workspace-scoping, or DDL.

use crate::bootstrap::system_request_metadata;
use crate::mount::{EngineServer, EngineState};
use anyhow::Context as _;
use lakekeeper::api::iceberg::types::{PageToken, Prefix};
use lakekeeper::api::iceberg::v1::namespace::{
    ListNamespacesQuery, NamespaceParameters, NamespaceService,
};
use lakekeeper::api::iceberg::v1::tables::{
    ListTablesQuery, LoadTableRequest, LoadTableResultOrNotModified, TableParameters, TablesService,
};
use lakekeeper::api::iceberg::v1::views::ViewService;
use lakekeeper::api::ApiContext;
use lakekeeper::iceberg::{NamespaceIdent, TableIdent};

/// A namespace as its multipart identifier (Iceberg namespaces can be nested).
pub struct NamespaceEntry {
    pub parts: Vec<String>,
}

pub enum TabularKind {
    Table,
    View,
}

pub struct TabularEntry {
    pub name: String,
    pub kind: TabularKind,
}

pub struct ColumnInfo {
    pub name: String,
    pub r#type: String,
    pub required: bool,
}

pub struct TableSchema {
    pub columns: Vec<ColumnInfo>,
    pub partition_by: Vec<String>,
    pub sort_by: Vec<String>,
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
) -> anyhow::Result<Vec<NamespaceEntry>> {
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
                return_uuids: false,
                return_protection_status: false,
            };
            let rm = system_request_metadata(system_user).context("system metadata")?;
            let resp = <EngineServer as NamespaceService<EngineState>>::list_namespaces(
                Some(px.clone()),
                query,
                engine_state.clone(),
                rm,
            )
            .await
            .map_err(|e| anyhow::anyhow!("list_namespaces: {e:?}"))?;
            for ns in resp.namespaces.iter() {
                let levels = ns.clone().inner();
                frontier.push(Some(
                    NamespaceIdent::from_vec(levels.clone())
                        .map_err(|e| anyhow::anyhow!("namespace ident: {e:?}"))?,
                ));
                out.push(NamespaceEntry { parts: levels });
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
) -> anyhow::Result<Vec<TabularEntry>> {
    let params = NamespaceParameters {
        prefix: Some(prefix(warehouse_id)),
        namespace: NamespaceIdent::from_vec(namespace.to_vec())
            .map_err(|e| anyhow::anyhow!("namespace ident: {e:?}"))?,
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
        .map_err(|e| anyhow::anyhow!("list_tables: {e:?}"))?;
        for ident in resp.identifiers.iter() {
            out.push(TabularEntry {
                name: ident.name.clone(),
                kind: TabularKind::Table,
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
        .map_err(|e| anyhow::anyhow!("list_views: {e:?}"))?;
        for ident in resp.identifiers.iter() {
            out.push(TabularEntry {
                name: ident.name.clone(),
                kind: TabularKind::View,
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
        return_uuids: false,
        return_protection_status: false,
    }
}

/// Load a table's current schema: columns (name/type/required) + the partition
/// and sort column names.
pub async fn load_table_schema(
    engine_state: ApiContext<EngineState>,
    system_user: &str,
    warehouse_id: &str,
    namespace: &[String],
    table: &str,
) -> anyhow::Result<TableSchema> {
    let ident = TableIdent {
        namespace: NamespaceIdent::from_vec(namespace.to_vec())
            .map_err(|e| anyhow::anyhow!("namespace ident: {e:?}"))?,
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
    .map_err(|e| anyhow::anyhow!("load_table: {e:?}"))?;

    let result = match loaded {
        LoadTableResultOrNotModified::LoadTableResult(res) => res,
        // We send no if-none-match etags, so the engine never returns 304.
        LoadTableResultOrNotModified::NotModifiedResponse(_) => {
            anyhow::bail!("unexpected 304 from load_table")
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
    let partition_by = md
        .default_partition_spec()
        .fields()
        .iter()
        .map(|pf| col_name(pf.source_id))
        .collect();
    let sort_by = md
        .default_sort_order()
        .fields
        .iter()
        .map(|sf| col_name(sf.source_id))
        .collect();

    Ok(TableSchema {
        columns,
        partition_by,
        sort_by,
    })
}
