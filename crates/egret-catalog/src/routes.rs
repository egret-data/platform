//! HTTP state + the Iceberg REST `iceberg_router()` (axum 0.8).

use crate::auth::require_bearer;
use crate::service::CatalogService;
use axum::routing::{get, post};
use axum::Router;
use egret_storage::StorageConfig;
use std::sync::Arc;

#[derive(Clone)]
pub struct CatalogState {
    pub service: CatalogService,
    pub auth_token: Arc<str>,
    pub storage_cfg: Arc<StorageConfig>,
    /// Base public URL of the catalog, e.g. `http://localhost:8080/catalog`, for signer.uri.
    pub base_url: Arc<str>,
}

pub fn iceberg_router(state: CatalogState) -> Router {
    Router::new()
        .route("/v1/config", get(crate::api::config::get_config))
        .route(
            "/v1/{prefix}/namespaces",
            get(crate::api::namespaces::list).post(crate::api::namespaces::create),
        )
        .route(
            "/v1/{prefix}/namespaces/{ns}",
            get(crate::api::namespaces::load)
                .head(crate::api::namespaces::head)
                .delete(crate::api::namespaces::drop),
        )
        .route(
            "/v1/{prefix}/namespaces/{ns}/properties",
            post(crate::api::namespaces::update_properties),
        )
        .route(
            "/v1/{prefix}/namespaces/{ns}/tables",
            get(crate::api::tables::list).post(crate::api::tables::create),
        )
        .route(
            "/v1/{prefix}/namespaces/{ns}/tables/{table}",
            get(crate::api::tables::load)
                .head(crate::api::tables::head)
                .post(crate::api::tables::commit)
                .delete(crate::api::tables::drop),
        )
        .route(
            "/v1/{prefix}/namespaces/{ns}/tables/{table}/metrics",
            post(crate::api::tables::metrics),
        )
        .route(
            "/v1/{prefix}/namespaces/{ns}/register",
            post(crate::api::tables::register),
        )
        .route(
            "/v1/{prefix}/tables/rename",
            post(crate::api::tables::rename),
        )
        .route(
            "/v1/{prefix}/namespaces/{ns}/views",
            get(crate::api::views::list).post(crate::api::views::create),
        )
        .route(
            "/v1/{prefix}/namespaces/{ns}/views/{view}",
            get(crate::api::views::load)
                .head(crate::api::views::head)
                .delete(crate::api::views::drop),
        )
        .route("/v1/{prefix}/aws/s3/sign", post(crate::api::sign::sign))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_bearer,
        ))
        .with_state(state)
}
