//! View handlers: list/create/load/head/drop.
//!
//! Views are stored with an opaque metadata JSON blob (no dedicated `egret-iceberg-ext`
//! DTO this phase), so requests/responses are handled as `serde_json::Value`.

use crate::api::tables::parse_namespace;
use crate::routes::CatalogState;
use crate::CatalogError;
use axum::extract::{Path, State};
use axum::Json;
use egret_iceberg_ext::{NamespaceIdent, TableIdent};
use serde_json::json;
use uuid::Uuid;

pub async fn list(
    State(s): State<CatalogState>,
    Path(ns): Path<String>,
) -> Result<Json<serde_json::Value>, CatalogError> {
    let levels = parse_namespace(&ns);
    let nsrow = s
        .service
        .store
        .get_namespace(s.service.catalog_id, &levels)
        .await?;
    let names = s.service.store.list_views(nsrow.id).await?;
    let ns_ident =
        NamespaceIdent::from_vec(levels).map_err(|e| CatalogError::BadRequest(e.to_string()))?;
    let identifiers: Vec<TableIdent> = names
        .into_iter()
        .map(|n| TableIdent::new(ns_ident.clone(), n))
        .collect();
    Ok(Json(json!({ "identifiers": identifiers })))
}

pub async fn create(
    State(s): State<CatalogState>,
    Path(ns): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, CatalogError> {
    let levels = parse_namespace(&ns);
    let nsrow = s
        .service
        .store
        .get_namespace(s.service.catalog_id, &levels)
        .await?;
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CatalogError::BadRequest("missing view name".into()))?
        .to_string();
    let view_uuid = Uuid::new_v4();
    let location = body
        .get("location")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| {
            egret_storage::table_base_location(&s.warehouse, &levels, &name, &view_uuid)
        });
    let mloc = egret_storage::table_metadata_location(&location, 0, &Uuid::new_v4());
    let bytes = serde_json::to_vec(&body).map_err(|e| CatalogError::BadRequest(e.to_string()))?;
    s.service.storage.put(&mloc, bytes).await?;
    s.service
        .store
        .insert_view(nsrow.id, &name, &body, &mloc, &location)
        .await?;
    Ok(Json(json!({
        "metadata-location": mloc,
        "metadata": body,
    })))
}

pub async fn load(
    State(s): State<CatalogState>,
    Path((ns, view)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, CatalogError> {
    let nsrow = s
        .service
        .store
        .get_namespace(s.service.catalog_id, &parse_namespace(&ns))
        .await?;
    let row = s.service.store.load_view(nsrow.id, &view).await?;
    Ok(Json(json!({
        "metadata-location": row.metadata_location,
        "metadata": row.metadata,
    })))
}

pub async fn head(
    State(s): State<CatalogState>,
    Path((ns, view)): Path<(String, String)>,
) -> Result<axum::http::StatusCode, CatalogError> {
    let nsrow = s
        .service
        .store
        .get_namespace(s.service.catalog_id, &parse_namespace(&ns))
        .await?;
    s.service.store.load_view(nsrow.id, &view).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

pub async fn drop(
    State(s): State<CatalogState>,
    Path((ns, view)): Path<(String, String)>,
) -> Result<axum::http::StatusCode, CatalogError> {
    let nsrow = s
        .service
        .store
        .get_namespace(s.service.catalog_id, &parse_namespace(&ns))
        .await?;
    s.service.store.drop_view(nsrow.id, &view).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}
