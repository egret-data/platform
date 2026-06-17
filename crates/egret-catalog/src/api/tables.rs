//! Table handlers: create/load/head/commit/drop/list/metrics/rename/register.

use crate::routes::CatalogState;
use crate::CatalogError;
use axum::extract::{Path, State};
use axum::Json;
use egret_iceberg_ext::catalog::{
    CommitTableRequest, CreateTableRequest, ListTablesResponse, LoadTableResult,
    RegisterTableRequest, RenameTableRequest,
};
use egret_iceberg_ext::{NamespaceIdent, TableIdent};

/// Decode a single `{ns}` path segment into namespace levels. Iceberg encodes
/// multipart namespaces with the unit separator `\u{1F}`; we also accept `.`.
pub fn parse_namespace(ns: &str) -> Vec<String> {
    ns.split('\u{1F}')
        .flat_map(|s| s.split('.'))
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

pub async fn create(
    State(s): State<CatalogState>,
    Path(ns): Path<String>,
    Json(req): Json<CreateTableRequest>,
) -> Result<Json<LoadTableResult>, CatalogError> {
    let levels = parse_namespace(&ns);
    Ok(Json(s.service.create_table(&levels, req).await?))
}

pub async fn load(
    State(s): State<CatalogState>,
    Path((ns, table)): Path<(String, String)>,
) -> Result<Json<LoadTableResult>, CatalogError> {
    Ok(Json(
        s.service.load_table(&parse_namespace(&ns), &table).await?,
    ))
}

pub async fn commit(
    State(s): State<CatalogState>,
    Path((ns, table)): Path<(String, String)>,
    Json(req): Json<CommitTableRequest>,
) -> Result<Json<LoadTableResult>, CatalogError> {
    Ok(Json(
        s.service
            .commit_table(&parse_namespace(&ns), &table, req)
            .await?,
    ))
}

pub async fn list(
    State(s): State<CatalogState>,
    Path(ns): Path<String>,
) -> Result<Json<ListTablesResponse>, CatalogError> {
    let levels = parse_namespace(&ns);
    let nsrow = s.service.store.get_namespace(&levels).await?;
    let names = s.service.store.list_tables(nsrow.id).await?;
    let ns_ident =
        NamespaceIdent::from_vec(levels).map_err(|e| CatalogError::BadRequest(e.to_string()))?;
    let identifiers = names
        .into_iter()
        .map(|n| TableIdent::new(ns_ident.clone(), n))
        .collect();
    Ok(Json(ListTablesResponse {
        next_page_token: None,
        identifiers,
    }))
}

pub async fn head(
    State(s): State<CatalogState>,
    Path((ns, table)): Path<(String, String)>,
) -> Result<axum::http::StatusCode, CatalogError> {
    let nsrow = s.service.store.get_namespace(&parse_namespace(&ns)).await?;
    s.service.store.load_table(nsrow.id, &table).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

pub async fn drop(
    State(s): State<CatalogState>,
    Path((ns, table)): Path<(String, String)>,
) -> Result<axum::http::StatusCode, CatalogError> {
    let nsrow = s.service.store.get_namespace(&parse_namespace(&ns)).await?;
    s.service.store.drop_table(nsrow.id, &table).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

pub async fn metrics(Json(_v): Json<serde_json::Value>) -> axum::http::StatusCode {
    axum::http::StatusCode::NO_CONTENT
}

pub async fn rename(
    State(s): State<CatalogState>,
    Json(req): Json<RenameTableRequest>,
) -> Result<axum::http::StatusCode, CatalogError> {
    let src_ns = s
        .service
        .store
        .get_namespace(req.source.namespace.as_ref())
        .await?;
    let dst_ns = s
        .service
        .store
        .get_namespace(req.destination.namespace.as_ref())
        .await?;
    s.service
        .store
        .rename_table(
            src_ns.id,
            &req.source.name,
            dst_ns.id,
            &req.destination.name,
        )
        .await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

pub async fn register(
    State(s): State<CatalogState>,
    Path(ns): Path<String>,
    Json(req): Json<RegisterTableRequest>,
) -> Result<Json<LoadTableResult>, CatalogError> {
    // Read the existing metadata.json from storage, then insert a row pointing at it.
    let levels = parse_namespace(&ns);
    let nsrow = s.service.store.get_namespace(&levels).await?;
    let bytes = s.service.storage.get(&req.metadata_location).await?;
    let md: iceberg::spec::TableMetadata =
        serde_json::from_slice(&bytes).map_err(|e| CatalogError::BadRequest(e.to_string()))?;
    let table_uuid = md.uuid();
    let location = md.location().to_string();
    s.service
        .store
        .insert_table(
            nsrow.id,
            &req.name,
            table_uuid,
            &md,
            &req.metadata_location,
            &location,
        )
        .await?;
    Ok(Json(LoadTableResult {
        metadata_location: Some(req.metadata_location),
        metadata: std::sync::Arc::new(md),
        config: None,
    }))
}
