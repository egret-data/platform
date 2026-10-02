//! Data Apps: the query gateway and its session tokens (spec docs/en/specs/2026-09-30-marimo-data-apps-design.md).
pub mod gateway;
pub mod grant;
pub mod podapi;
pub mod podtoken;
pub mod readonly;
pub mod routes;
pub mod session;
pub mod store;
pub mod template;

pub use gateway::{gateway_router, QueryRequest};
pub use podapi::{pod_router, PodApiState};
pub use routes::{apps_router, AppsApiState};

/// OpenAPI fragment for the cookie-session app API (`/api/ws/{workspace}/apps…`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi;
    routes::AppsApi::openapi()
}

/// Everything `POST /api/apps/query` needs. Built by the server binary.
#[derive(Clone)]
pub struct AppsState {
    pub db: egret_metastore::Db,
    pub pdp: std::sync::Arc<dyn egret_core::authz::Pdp>,
    pub query: std::sync::Arc<dyn egret_query_api::QueryExecPort>,
    pub audit: egret_audit::AuditSink,
    pub sessions: session::AppSessionRepo,
    /// Keyed per app session (token), so one runaway notebook throttles only itself.
    pub limiter: egret_core::ratelimit::RateLimiter,
    pub cfg: egret_config::AppsConfig,
}
