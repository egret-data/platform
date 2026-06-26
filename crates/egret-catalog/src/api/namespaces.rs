//! Namespace handlers: create/load/head/drop/list/update_properties.

use crate::api::resolve::{resolve_catalog, AuthnPrincipal};
use crate::api::tables::parse_namespace;
use crate::routes::CatalogState;
use crate::CatalogError;
use axum::extract::{Path, Query, State};
use axum::{Extension, Json};
use egret_iceberg_ext::catalog::{
    CreateNamespaceRequest, CreateNamespaceResponse, GetNamespaceResponse, ListNamespacesResponse,
    UpdateNamespacePropertiesRequest, UpdateNamespacePropertiesResponse,
};
use egret_iceberg_ext::NamespaceIdent;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize)]
pub struct ListQuery {
    pub parent: Option<String>,
}

pub async fn list(
    State(s): State<CatalogState>,
    Extension(principal): Extension<AuthnPrincipal>,
    Path(prefix): Path<String>,
    Query(q): Query<ListQuery>,
) -> Result<Json<ListNamespacesResponse>, CatalogError> {
    let cat = resolve_catalog(&s, &principal, &prefix).await?;
    let parent = q.parent.as_deref().map(parse_namespace);
    let rows = s
        .service
        .store
        .list_namespaces(cat.catalog_id, parent.as_deref())
        .await?;
    let mut namespaces = Vec::with_capacity(rows.len());
    for levels in rows {
        namespaces.push(
            NamespaceIdent::from_vec(levels)
                .map_err(|e| CatalogError::BadRequest(e.to_string()))?,
        );
    }
    Ok(Json(ListNamespacesResponse {
        next_page_token: None,
        namespaces,
    }))
}

pub async fn create(
    State(s): State<CatalogState>,
    Extension(principal): Extension<AuthnPrincipal>,
    Path(prefix): Path<String>,
    Json(req): Json<CreateNamespaceRequest>,
) -> Result<Json<CreateNamespaceResponse>, CatalogError> {
    let cat = resolve_catalog(&s, &principal, &prefix).await?;
    let levels: Vec<String> = req.namespace.as_ref().clone();
    let props = req.properties.clone().unwrap_or_default();
    let row = s
        .service
        .store
        .create_namespace(cat.catalog_id, &levels, &props)
        .await?;
    let properties: HashMap<String, String> =
        serde_json::from_value(row.properties).unwrap_or_default();
    Ok(Json(CreateNamespaceResponse {
        namespace: req.namespace,
        properties: Some(properties),
    }))
}

pub async fn load(
    State(s): State<CatalogState>,
    Extension(principal): Extension<AuthnPrincipal>,
    Path((prefix, ns)): Path<(String, String)>,
) -> Result<Json<GetNamespaceResponse>, CatalogError> {
    let cat = resolve_catalog(&s, &principal, &prefix).await?;
    let levels = parse_namespace(&ns);
    let row = s
        .service
        .store
        .get_namespace(cat.catalog_id, &levels)
        .await?;
    let properties: HashMap<String, String> =
        serde_json::from_value(row.properties).unwrap_or_default();
    let namespace = NamespaceIdent::from_vec(row.levels)
        .map_err(|e| CatalogError::BadRequest(e.to_string()))?;
    Ok(Json(GetNamespaceResponse {
        namespace,
        properties: Some(properties),
    }))
}

pub async fn head(
    State(s): State<CatalogState>,
    Extension(principal): Extension<AuthnPrincipal>,
    Path((prefix, ns)): Path<(String, String)>,
) -> Result<axum::http::StatusCode, CatalogError> {
    let cat = resolve_catalog(&s, &principal, &prefix).await?;
    s.service
        .store
        .get_namespace(cat.catalog_id, &parse_namespace(&ns))
        .await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

pub async fn drop(
    State(s): State<CatalogState>,
    Extension(principal): Extension<AuthnPrincipal>,
    Path((prefix, ns)): Path<(String, String)>,
) -> Result<axum::http::StatusCode, CatalogError> {
    let cat = resolve_catalog(&s, &principal, &prefix).await?;
    s.service
        .store
        .drop_namespace(cat.catalog_id, &parse_namespace(&ns))
        .await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

pub async fn update_properties(
    State(s): State<CatalogState>,
    Extension(principal): Extension<AuthnPrincipal>,
    Path((prefix, ns)): Path<(String, String)>,
    Json(req): Json<UpdateNamespacePropertiesRequest>,
) -> Result<Json<UpdateNamespacePropertiesResponse>, CatalogError> {
    let cat = resolve_catalog(&s, &principal, &prefix).await?;
    let levels = parse_namespace(&ns);
    let removals = req.removals.unwrap_or_default();
    let updates = req.updates.unwrap_or_default();
    let (updated, removed) = s
        .service
        .store
        .update_namespace_properties(cat.catalog_id, &levels, &removals, &updates)
        .await?;
    // Report removals requested but not present as `missing`.
    let missing: Vec<String> = removals
        .into_iter()
        .filter(|k| !removed.contains(k))
        .collect();
    Ok(Json(UpdateNamespacePropertiesResponse {
        updated,
        removed,
        missing: if missing.is_empty() {
            None
        } else {
            Some(missing)
        },
    }))
}
