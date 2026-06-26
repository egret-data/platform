//! View handlers: list/create/load/head/drop.
//!
//! Views are stored normalized (migration 0023). The create handler builds a
//! typed [`iceberg::spec::ViewMetadata`] from the Iceberg REST `CreateViewRequest`
//! and shreds it; load assembles it back and serializes the typed metadata.

use crate::api::tables::parse_namespace;
use crate::routes::CatalogState;
use crate::CatalogError;
use axum::extract::{Path, State};
use axum::Json;
use egret_iceberg_ext::{NamespaceIdent, TableIdent};
use iceberg::spec::{
    Schema, SqlViewRepresentation, ViewFormatVersion, ViewMetadata, ViewMetadataBuilder,
    ViewRepresentation, ViewRepresentations, ViewVersion,
};
use serde_json::json;
use std::collections::HashMap;
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

    // Build a typed ViewMetadata (format v1) from the CreateView request.
    let view_md = build_view_metadata(&body, &location, view_uuid)?;

    let mloc = egret_storage::table_metadata_location(&location, 0, &Uuid::new_v4());
    let metadata_json =
        serde_json::to_value(&view_md).map_err(|e| CatalogError::BadRequest(e.to_string()))?;
    let bytes =
        serde_json::to_vec(&metadata_json).map_err(|e| CatalogError::BadRequest(e.to_string()))?;
    s.service.storage.put(&mloc, bytes).await?;
    s.service
        .store
        .insert_view(nsrow.id, &name, &view_md, &mloc, &location)
        .await?;
    Ok(Json(json!({
        "metadata-location": mloc,
        "metadata": metadata_json,
    })))
}

/// Construct a v1 [`ViewMetadata`] from an Iceberg REST `CreateViewRequest` body.
///
/// The request carries: `schema`, `view-version` (with `representations`,
/// `default-namespace`, optional `default-catalog`, `summary`), and `properties`.
/// We assign the freshly-generated `view_uuid` and force format-version 1.
fn build_view_metadata(
    body: &serde_json::Value,
    location: &str,
    view_uuid: Uuid,
) -> Result<ViewMetadata, CatalogError> {
    let schema_val = body
        .get("schema")
        .ok_or_else(|| CatalogError::BadRequest("missing view schema".into()))?;
    let schema: Schema = serde_json::from_value(schema_val.clone())
        .map_err(|e| CatalogError::BadRequest(format!("invalid view schema: {e}")))?;

    let vv = body
        .get("view-version")
        .ok_or_else(|| CatalogError::BadRequest("missing view-version".into()))?;

    // Representations: [{ type: "sql", sql, dialect }, ...] — required, non-empty.
    let arr = vv
        .get("representations")
        .and_then(|v| v.as_array())
        .filter(|a| !a.is_empty())
        .ok_or_else(|| {
            CatalogError::BadRequest(
                "view-version.representations must be a non-empty array".into(),
            )
        })?;
    let mut reps: Vec<ViewRepresentation> = Vec::with_capacity(arr.len());
    for rep in arr {
        let sql = rep
            .get("sql")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CatalogError::BadRequest("representation missing sql".into()))?
            .to_string();
        let dialect = rep
            .get("dialect")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CatalogError::BadRequest("representation missing dialect".into()))?
            .to_string();
        reps.push(ViewRepresentation::Sql(SqlViewRepresentation {
            sql,
            dialect,
        }));
    }
    let representations = ViewRepresentations::builder()
        .add_all_representations(reps)
        .build()
        .map_err(|e| CatalogError::BadRequest(e.to_string()))?;

    let summary: HashMap<String, String> = vv
        .get("summary")
        .map(|v| serde_json::from_value(v.clone()))
        .transpose()
        .map_err(|e| CatalogError::BadRequest(format!("invalid view-version summary: {e}")))?
        .unwrap_or_default();

    let default_catalog = vv
        .get("default-catalog")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let default_namespace: NamespaceIdent = match vv.get("default-namespace") {
        Some(v) => serde_json::from_value(v.clone())
            .map_err(|e| CatalogError::BadRequest(format!("invalid default-namespace: {e}")))?,
        None => crate::service::view_repo::empty_namespace_ident(),
    };

    let timestamp_ms = vv
        .get("timestamp-ms")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

    let version = ViewVersion::builder()
        .with_version_id(1)
        .with_schema_id(schema.schema_id())
        .with_timestamp_ms(timestamp_ms)
        .with_summary(summary)
        .with_representations(representations)
        .with_default_catalog(default_catalog)
        .with_default_namespace(default_namespace)
        .build();

    let properties: HashMap<String, String> = body
        .get("properties")
        .map(|v| serde_json::from_value(v.clone()))
        .transpose()
        .map_err(|e| CatalogError::BadRequest(format!("invalid view properties: {e}")))?
        .unwrap_or_default();

    let build = ViewMetadataBuilder::new(
        location.to_string(),
        schema,
        version,
        ViewFormatVersion::V1,
        properties,
    )
    .map_err(|e| CatalogError::BadRequest(e.to_string()))?
    .assign_uuid(view_uuid)
    .build()
    .map_err(|e| CatalogError::BadRequest(e.to_string()))?;

    Ok(build.metadata)
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
    let metadata_json =
        serde_json::to_value(&row.metadata).map_err(|e| CatalogError::BadRequest(e.to_string()))?;
    Ok(Json(json!({
        "metadata-location": row.metadata_location,
        "metadata": metadata_json,
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
