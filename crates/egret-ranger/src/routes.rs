//! HTTP state + the Ranger Admin `ranger_router()` (axum 0.8).

use crate::api;
use axum::routing::get;
use axum::Router;
use egret_authz::AuthzStore;
use std::sync::Arc;

#[derive(Clone)]
pub struct RangerState {
    pub store: AuthzStore,
    pub auth_token: Arc<str>,
}

pub fn ranger_router(state: RangerState) -> Router {
    Router::new()
        .route(
            "/plugins/policies/download/{service}",
            get(api::download::download_policies),
        )
        .route(
            "/tags/download/{service}",
            get(api::download::download_tags),
        )
        .merge(api::crud_routes())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::auth::require_bearer,
        ))
        .with_state(state)
}
