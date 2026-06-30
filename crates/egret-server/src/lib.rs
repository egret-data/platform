//! HTTP composition root: shared application state, the router, and health endpoints.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use egret_auth::oidc_routes::{oidc_router, OidcRoutesState};
use egret_auth::routes::{auth_router, AuthState};
use egret_metastore::Db;
use egret_workspace::routes::{
    admin_user_router, admin_workspace_router, me_router, ws_member_router, WorkspaceState,
};

/// Application state shared by all handlers.
#[derive(Clone)]
pub struct AppState {
    /// `None` in unit tests; `Some` once a database is wired in `egret-bin`.
    pub db: Option<Db>,
    /// Present once a database is wired (in `egret-bin`); `None` in pure health unit tests.
    pub auth: Option<AuthState>,
    /// Present once workspace crate is wired; `None` in pure health unit tests.
    pub workspace: Option<WorkspaceState>,
    /// Present when `config.oidc.enabled`; `None` otherwise (or in unit tests).
    pub oidc: Option<OidcRoutesState>,
    /// The LIVE embedded lakekeeper Iceberg REST engine router, fully assembled
    /// in the composition root (`egret-bin`): the lakekeeper catalog router
    /// fronted by egret's per-workspace token-ownership middleware (CE-2 T5).
    /// Carried as a pre-built `axum::Router` so `egret-server` stays free of the
    /// lakekeeper engine graph (the engine pulls in a second `sqlx` major). Nested
    /// at `/catalog/v1`. `Some` once `config.catalog.enabled` wires the engine;
    /// `None` otherwise (or in unit tests).
    pub catalog_engine: Option<axum::Router>,
    /// Session-authenticated internal catalog browse API (for the UI), reading
    /// the embedded lakekeeper catalog (CE-2 T6). Like `catalog_engine`, it is
    /// assembled in the composition root (`egret-bin`) and carried as a pre-built
    /// `axum::Router` so `egret-server` stays free of the lakekeeper engine graph
    /// (the engine pulls in a second `sqlx` major). Nested at `/api`, fronted by
    /// the CSRF layer. `Some` once a database + the engine are wired; `None`
    /// otherwise (or in unit tests).
    pub catalog_browse: Option<axum::Router>,
    /// Present when `config.ranger.enabled`; `None` otherwise (or in unit tests).
    pub ranger: Option<egret_ranger::RangerState>,
    /// Present once a database is wired (in `egret-bin`); `None` in pure health unit tests.
    pub database: Option<egret_database::DatabaseState>,
    /// Present once the engine is wired (in `egret-bin`); `None` in unit tests.
    pub engine: Option<egret_engine::EngineState>,
    /// Present once the BI layer is wired (in `egret-bin`); `None` in unit tests.
    pub bi: Option<egret_bi::BiState>,
    /// Present once the notebook layer is wired (in `egret-bin`); `None` in unit tests.
    pub notebook: Option<egret_notebook::NotebookState>,
    /// Present once the kernel layer is wired (in `egret-bin`); `None` in unit tests.
    pub kernel: Option<egret_kernel::KernelState>,
}

impl AppState {
    /// Build state with no database — used by unit tests and `--no-db` boots.
    pub fn without_db() -> Self {
        Self {
            db: None,
            auth: None,
            workspace: None,
            oidc: None,
            catalog_engine: None,
            catalog_browse: None,
            ranger: None,
            database: None,
            engine: None,
            bi: None,
            notebook: None,
            kernel: None,
        }
    }
}

/// Build the root router. Later phases mount feature routers here.
pub fn build_router(state: AppState) -> Router {
    let mut router = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz));

    if let Some(auth) = state.auth.clone() {
        router = router.merge(
            Router::new()
                .nest("/api/auth", auth_router())
                .with_state(auth),
        );
    }

    if let Some(ws) = state.workspace.clone() {
        let admin = Router::new()
            .nest(
                "/api/admin",
                admin_workspace_router().merge(admin_user_router()),
            )
            .nest("/api/ws", ws_member_router())
            .nest("/api/me", me_router())
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(ws);
        router = router.merge(admin);
    }

    if let Some(oidc) = state.oidc.clone() {
        router = router.merge(
            Router::new()
                .nest("/api/auth/oidc", oidc_router())
                .with_state(oidc),
        );
    }

    // CE-2 cutover: the live catalog surface is the embedded lakekeeper Iceberg
    // REST engine (`state.catalog_engine`), assembled in `egret-bin` with egret's
    // per-workspace token-ownership middleware in front (A-3), nested at
    // `/catalog/v1` so the front middleware sees lakekeeper-relative paths
    // (`/{prefix}/...` and `/config`).
    if let Some(catalog_engine) = state.catalog_engine.clone() {
        router = router.merge(
            Router::new()
                .nest("/catalog/v1", catalog_engine)
                .with_state(()),
        );
    }

    // CE-2 T6: the browse API now reads the embedded lakekeeper catalog (not
    // egret's stale `catalog_*` tables). It is assembled in `egret-bin`
    // (`egret_catalog_engine::browse_router` with its `BrowseState`) and carried
    // here as a pre-built `axum::Router` so this crate stays off the engine graph.
    // Mounted at `/api`, fronted by the CSRF layer (same as before).
    if let Some(browse) = state.catalog_browse.clone() {
        let group = Router::new()
            .nest("/api", browse)
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(());
        router = router.merge(group);
    }

    if let Some(ranger) = state.ranger.clone() {
        router = router.merge(
            Router::new()
                .nest("/service", egret_ranger::ranger_router(ranger))
                .with_state(()),
        );
    }

    if let Some(database) = state.database.clone() {
        let group = Router::new()
            .nest("/api", egret_database::workspace_database_router())
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(database);
        router = router.merge(group);
    }

    if let Some(engine) = state.engine.clone() {
        let group = Router::new()
            .nest("/api", egret_engine::engine_router())
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(engine);
        router = router.merge(group);
    }

    if let Some(bi) = state.bi.clone() {
        let bi_routes = egret_bi::sqllab_router()
            .merge(egret_bi::dataset_router())
            .merge(egret_bi::chart_router())
            .merge(egret_bi::dashboard_router())
            .merge(egret_bi::folder_router());
        let group = Router::new()
            .nest("/api", bi_routes)
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(bi);
        router = router.merge(group);
    }

    if let Some(notebook) = state.notebook.clone() {
        let group = Router::new()
            .nest("/api", egret_notebook::notebook_router())
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(notebook);
        router = router.merge(group);
    }

    if let Some(kernel) = state.kernel.clone() {
        // Session-cookie kernel lifecycle/channels routes: CSRF-protected.
        let session_group = Router::new()
            .nest("/api", egret_kernel::kernel_router())
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(kernel.clone());
        router = router.merge(session_group);
        // Kernel-token bearer callbacks (invoked by the kernel subprocess, which
        // has no browser session/CSRF cookie): CSRF-exempt, like /catalog/v1 and
        // the Ranger /service routes. Auth is the EGRET_KERNEL_TOKEN bearer.
        let callback_group = Router::new()
            .nest("/api", egret_kernel::kernel_callback_router())
            .with_state(kernel);
        router = router.merge(callback_group);
    }

    router.with_state(state)
}

/// Liveness: the process is up. Always 200.
async fn healthz() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

/// Readiness: dependencies are reachable. 200 when the DB (if any) responds.
async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    match &state.db {
        None => (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "ok", "db": "none" })),
        ),
        Some(db) => match db.ping().await {
            Ok(()) => (
                StatusCode::OK,
                Json(serde_json::json!({ "status": "ok", "db": "up" })),
            ),
            Err(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "status": "degraded", "db": "down" })),
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt; // for `oneshot`

    #[tokio::test]
    async fn healthz_returns_200_ok() {
        let app = build_router(AppState::without_db());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["status"], "ok");
    }

    #[tokio::test]
    async fn readyz_ok_without_db() {
        let app = build_router(AppState::without_db());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
