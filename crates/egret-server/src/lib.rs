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
    access_request_router, admin_user_router, admin_workspace_router, me_router, ws_member_router,
    WorkspaceState,
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
    /// Present when `config.ranger.enabled`; `None` otherwise (or in unit tests).
    pub ranger: Option<egret_policy::RangerState>,
    /// Present once a database is wired (in `egret-bin`); `None` in pure health unit tests.
    pub database: Option<egret_query::DatabaseState>,
    /// Present once the engine is wired (in `egret-bin`); `None` in unit tests.
    pub engine: Option<egret_query::EngineState>,
    /// Present once the BI layer is wired (in `egret-bin`); `None` in unit tests.
    pub bi: Option<egret_bi::BiState>,
    /// Present once the notebook layer is wired (in `egret-bin`); `None` in unit tests.
    pub notebook: Option<egret_notebook::NotebookState>,
    /// Present once the kernel layer is wired (in `egret-bin`); `None` in unit tests.
    pub kernel: Option<egret_kernel::KernelState>,
    /// Present once the control plane is wired (in `egret-bin`); `None` in unit tests.
    pub control_plane: Option<egret_controlplane::CpState>,
    /// Present once the Catalog seam admin router is wired (E3d-3); `None` in unit tests.
    pub catalog_admin: Option<egret_catalog_engine::CatalogAdminState>,
    /// Present once the Policy/Ranger admin router is wired (E2d); `None` in unit tests.
    pub policy_admin: Option<egret_policy::PolicyAdminState>,
    /// Present once the engine-mgmt seam is wired (in `egret-bin`); `None` in unit tests.
    pub engine_mgmt: Option<egret_engine_mgmt::EngineMgmtState>,
    /// Embedded lakekeeper Iceberg REST catalog router, mounted at `/catalog/v1`
    /// when `config.catalog.enabled` (in `egret-bin`); `None` otherwise.
    pub catalog_engine: Option<axum::Router>,
    /// egret mini machine-IdP (mints Catalog Client tokens); `None` in unit tests.
    pub idp: Option<egret_machine_idp::IdpState>,
}

impl AppState {
    /// Build state with no database — used by unit tests and `--no-db` boots.
    pub fn without_db() -> Self {
        Self {
            db: None,
            auth: None,
            workspace: None,
            oidc: None,
            ranger: None,
            database: None,
            engine: None,
            bi: None,
            notebook: None,
            kernel: None,
            control_plane: None,
            catalog_admin: None,
            policy_admin: None,
            engine_mgmt: None,
            catalog_engine: None,
            idp: None,
        }
    }
}

/// The aggregated OpenAPI 3.1 document for the whole API (E0·P4).
///
/// Starts from a base doc (title + version) and merges each annotated seam's
/// `openapi()`. This is the single extension point P4b grows: add one
/// `doc.merge(egret_x::openapi());` line per newly-annotated crate. Pure — no DB
/// or network — so the `egret openapi` subcommand can emit it offline.
///
/// # Conventions P4b must follow (locked in P4a's Query-seam reference)
///
/// - **Keep the `.merge()` list explicit** — one line per crate, do NOT loop.
///   The order is the doc's tag order and stays readable/reviewable.
/// - **Component names are GLOBAL after merge.** utoipa keys schemas by their
///   Rust type name; two crates that both export a `Config`/`Status` will
///   collide and silently overwrite each other in the merged doc. Keep schema
///   type names unique across crates (prefix with the seam if needed).
/// - **Standard error responses.** Every handler lists the two invariant error
///   responses — `(status = 401, body = ErrorBody, "Not authenticated")` and
///   `(status = 500, body = ErrorBody, "Internal error")` — plus any
///   hand-written `403` / `404` / `409` with a *meaningful, endpoint-specific*
///   description. The varying descriptions are intentional API documentation,
///   not boilerplate to factor away; only the 401/500 pair is mechanical.
/// - **Shared shapes** live in `egret_core`: [`egret_core::WorkspacePath`] for
///   the `{workspace}` path param (`params(WorkspacePath, …)`), [`egret_core::Ack`]
///   for `{ "ok": true }` bodies, [`egret_core::http::ErrorBody`] for errors,
///   and [`egret_core::ColumnMeta`] for tabular column metadata.
pub fn openapi_doc() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi as _;

    /// Base document: API-wide metadata only; every path/schema comes from a
    /// merged seam doc.
    #[derive(utoipa::OpenApi)]
    #[openapi(info(
        title = "egret API",
        version = "0.1.0",
        description = "egret unified data portal HTTP API."
    ))]
    struct BaseApiDoc;

    let mut doc = BaseApiDoc::openapi();
    doc.merge(egret_query::openapi());
    doc.merge(egret_bi::openapi());
    doc.merge(egret_auth::openapi());
    doc.merge(egret_workspace::openapi());
    doc.merge(egret_notebook::openapi());
    doc.merge(egret_kernel::openapi());
    doc.merge(egret_controlplane::openapi());
    doc.merge(egret_catalog_engine::openapi());
    doc.merge(egret_policy::openapi());
    doc.merge(egret_engine_mgmt::openapi());
    doc.merge(egret_machine_idp::openapi());
    doc
}

/// Serve the aggregated OpenAPI document as JSON. Mounted unauthenticated and
/// outside the CSRF/auth layers (dev doc surface; consumed by FE codegen).
async fn openapi_json() -> Json<utoipa::openapi::OpenApi> {
    Json(openapi_doc())
}

/// Build the root router. Later phases mount feature routers here.
pub fn build_router(state: AppState) -> Router {
    let mut router = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        // OpenAPI spec: no auth/CSRF so FE codegen + tooling can fetch it.
        .route("/api-docs/openapi.json", get(openapi_json));

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
            .nest("/api", access_request_router())
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

    if let Some(ranger) = state.ranger.clone() {
        router = router.merge(
            Router::new()
                .nest("/service", egret_policy::ranger_router(ranger))
                .with_state(()),
        );
    }

    if let Some(database) = state.database.clone() {
        let group = Router::new()
            .nest("/api", egret_query::workspace_database_router())
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(database);
        router = router.merge(group);
    }

    if let Some(engine) = state.engine.clone() {
        let group = Router::new()
            .nest("/api", egret_query::engine_router())
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(engine);
        router = router.merge(group);
    }

    if let Some(bi) = state.bi.clone() {
        let bi_routes = egret_bi::sqllab_router()
            .merge(egret_bi::dataset_router())
            .merge(egret_bi::chart_router())
            .merge(egret_bi::dashboard_router())
            .merge(egret_bi::folder_router())
            .merge(egret_bi::shares_router())
            .merge(egret_bi::resource_tags_router());
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

    if let Some(cp) = state.control_plane.clone() {
        // Global platform-admin Storage + Secrets: CSRF-protected like the other
        // mutating APIs.
        let group = Router::new()
            .nest(
                "/api/control-plane",
                egret_controlplane::control_plane_router(),
            )
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(cp);
        router = router.merge(group);
    }

    if let Some(ca) = state.catalog_admin.clone() {
        // Catalog seam admin router (E3d-3), moved out of control_plane_router.
        // Same /api/control-plane prefix (FE URLs unchanged) + CSRF layer.
        let group = Router::new()
            .nest(
                "/api/control-plane",
                egret_catalog_engine::catalog_admin_router(),
            )
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(ca);
        router = router.merge(group);
    }

    if let Some(pa) = state.policy_admin.clone() {
        // Policy/Ranger admin console (E2d), moved out of control_plane_router.
        // Same /api/control-plane prefix (FE URLs unchanged) + CSRF layer.
        let group = Router::new()
            .nest("/api/control-plane", egret_policy::policy_admin_router())
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(pa);
        router = router.merge(group);
    }

    if let Some(em) = state.engine_mgmt.clone() {
        let group = Router::new()
            .nest(
                "/api/control-plane",
                egret_engine_mgmt::engine_mgmt_router(),
            )
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(em);
        router = router.merge(group);
    }

    if let Some(catalog_engine) = state.catalog_engine.clone() {
        // Embedded lakekeeper Iceberg REST catalog, nested at /catalog/v1. Machine
        // API — no CSRF/session auth here (EB-1: AllowAll; EB-3 adds JWT authN +
        // the egret Authorizer). The router is lakekeeper-relative (/config, /{prefix}/…).
        router = router.nest_service("/catalog/v1", catalog_engine);
    }

    if let Some(idp) = state.idp.clone() {
        // egret mini machine-IdP: /oauth/token + /.well-known/*. Public machine
        // endpoints — no CSRF, no session auth.
        router = router.merge(egret_machine_idp::idp_router().with_state(idp));
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

    /// Drift gate: the committed repo-root `openapi.json` must be byte-identical
    /// to a fresh emit. `preserve_order` makes the emit deterministic, so this
    /// won't flap. It fails the moment someone changes a path/schema and forgets
    /// to regenerate — guarding P4b against shipping stale FE types. Serialization
    /// matches the `egret openapi` subcommand exactly (pretty + trailing newline).
    #[test]
    fn committed_openapi_json_is_current() {
        let emitted = format!(
            "{}\n",
            serde_json::to_string_pretty(&openapi_doc()).unwrap()
        );
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../openapi.json");
        let committed = std::fs::read_to_string(path).expect("openapi.json missing at repo root");
        assert_eq!(
            committed, emitted,
            "openapi.json is stale — run `egret openapi > openapi.json`"
        );
    }

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
