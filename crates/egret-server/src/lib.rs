//! HTTP composition root: shared application state, the router, and health endpoints.

pub mod metrics;

pub use metrics::install_recorder as install_metrics_recorder;

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
use std::future::IntoFuture;
use std::time::Duration;

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
    /// Present once the Alerts & Reports seam is wired (in `egret-bin`); `None` in unit tests.
    pub automation: Option<egret_automation::AutomationState>,
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
    /// Data-permission model surface (SP4a Task 9): grants, rules, `explain`,
    /// and the manual compile trigger. `None` in unit tests.
    pub perm: Option<egret_perm_compile::PermState>,
    /// Present once the engine-mgmt seam is wired (in `egret-bin`); `None` in unit tests.
    pub engine_mgmt: Option<egret_engine_mgmt::EngineMgmtState>,
    /// Embedded lakekeeper Iceberg REST catalog router, mounted at `/catalog/v1`
    /// when `config.catalog.enabled` (in `egret-bin`); `None` otherwise.
    pub catalog_engine: Option<axum::Router>,
    /// egret mini machine-IdP (mints Catalog Client tokens); `None` in unit tests.
    pub idp: Option<egret_machine_idp::IdpState>,
    /// Data API management surface (P2): service accounts and their OAuth2
    /// clients; `None` in unit tests.
    pub dataapi: Option<egret_dataapi::DataApiState>,
    /// Workflow management surface (W1 Task 7): flow drafts and their
    /// published versions; `None` in unit tests.
    pub workflow: Option<egret_workflow::WorkflowState>,
    /// **The exposure lever for the anonymous webhook surface (W6 Task 7).
    /// `false` means the routes are not mounted at all.**
    ///
    /// `POST /workflows/v1/{ws}/executions/webhook/{namespace}/{flow}/{key}`
    /// is unauthenticated by construction — the key in the path is the whole
    /// credential — and its own security review's verdict is *not approved for
    /// exposure*. Until fix round 2 that verdict had **no lever**: the router
    /// was mounted unconditionally, so an operator who had read the warning
    /// could do nothing about it. A documented limitation nobody can act on is
    /// not a control.
    ///
    /// **Default `false`**, in [`AppState::without_db`] and in
    /// `egret_config::WorkflowConfig`, which `egret-bin` reads as
    /// `[workflow] webhooks_enabled` / `EGRET_WORKFLOW__WEBHOOKS_ENABLED` —
    /// the `EGRET_<SECTION>__<KEY>` convention, and a key the helm chart sets.
    /// Off is the only safe default for a surface whose own doc says it should
    /// not be exposed; an operator who wants it says so once, deliberately, and
    /// a value figment cannot parse is a startup error rather than a silent
    /// `false`.
    ///
    /// It gates **only** the webhook group. Workflow management
    /// (`/api/ws/{workspace}/workflows/...`, session-authenticated) is
    /// unaffected: turning this off stops webhook traffic and nothing else,
    /// which is the same split `flows.state = 'disabled'` makes per flow.
    pub workflow_webhooks_enabled: bool,
    /// Data Apps query gateway (`POST /api/apps/query`), called with a
    /// per-marimo-session bearer token from inside app pods; `None` in unit tests.
    pub apps: Option<egret_apps::AppsState>,
    /// Data Apps management API (`/api/ws/{workspace}/apps…`): cookie session,
    /// behind CSRF; `None` in unit tests.
    pub apps_api: Option<egret_apps::AppsApiState>,
    /// Data Apps pod API (`/api/apps/pod/…`): a pod token, no cookie, so no CSRF
    /// layer; `None` in unit tests.
    pub apps_pod: Option<egret_apps::PodApiState>,
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
            automation: None,
            notebook: None,
            kernel: None,
            control_plane: None,
            catalog_admin: None,
            policy_admin: None,
            perm: None,
            engine_mgmt: None,
            catalog_engine: None,
            idp: None,
            dataapi: None,
            workflow: None,
            // Off. See the field's doc: this is the lever for a surface whose
            // security review says "do not expose", so the default has to be
            // the side of that sentence.
            workflow_webhooks_enabled: false,
            apps: None,
            apps_api: None,
            apps_pod: None,
        }
    }

    /// Attach the Data API seam to an otherwise-empty test state.
    ///
    /// Exists because no test had ever built the COMPOSED router with this group
    /// merged: [`Self::without_db`] leaves `dataapi: None`, so
    /// `build_router(without_db())` mounts nothing of it. That gap matters for
    /// one property in particular — whether the serving group inherited the CSRF
    /// layer. A test that calls `serving_router()` directly never mounts the
    /// layer and therefore passes whether or not the group carries it, which is
    /// exactly how `render_router` shipped a POST that was rejected in
    /// production and green in CI (`egret-automation/src/routes.rs:115-126`).
    /// Only the composed router can answer that question.
    pub fn with_dataapi(mut self, dataapi: egret_dataapi::DataApiState) -> Self {
        self.dataapi = Some(dataapi);
        self
    }

    /// Attach the workflow seam to an otherwise-empty test state.
    ///
    /// Exists for the same reason [`Self::with_dataapi`] does, and against the
    /// same class of bug: [`Self::without_db`] leaves `workflow: None`, so
    /// `build_router(without_db())` mounts neither the management group nor the
    /// webhook group, and no test had ever built the composed router with the
    /// **anonymous webhook surface** merged. That surface is the one whose
    /// layering can be wrong in a way its own suite cannot see — it must land
    /// OUTSIDE the CSRF layer, and `egret-workflow`'s `tests/webhook_route.rs`
    /// mounts `webhook_router()` directly, so it passes whether or not
    /// `build_router` wraps it.
    pub fn with_workflow(mut self, workflow: egret_workflow::WorkflowState) -> Self {
        self.workflow = Some(workflow);
        self
    }

    /// Turn the anonymous webhook surface ON for a test.
    ///
    /// Separate from [`Self::with_workflow`] on purpose: a test that wants the
    /// management router must not get the anonymous one as a side effect, and
    /// the one test file that drives the webhook mount has to say so in its own
    /// source. See [`AppState::workflow_webhooks_enabled`].
    #[must_use]
    pub fn with_workflow_webhooks(mut self) -> Self {
        self.workflow_webhooks_enabled = true;
        self
    }

    /// Attach the Data Apps gateway to an otherwise-empty test state, so a test
    /// can prove through the COMPOSED router that the gateway is mounted and is
    /// not behind the CSRF layer (same reasoning as [`Self::with_dataapi`]).
    pub fn with_apps(mut self, apps: egret_apps::AppsState) -> Self {
        self.apps = Some(apps);
        self
    }

    /// Attach the Data Apps management API to an otherwise-empty test state, so a
    /// test can prove through the COMPOSED router that it is mounted and IS behind
    /// the CSRF layer — the opposite of the gateway.
    pub fn with_apps_api(mut self, apps_api: egret_apps::AppsApiState) -> Self {
        self.apps_api = Some(apps_api);
        self
    }

    /// Attach the Data Apps pod API to an otherwise-empty test state, so a test can
    /// prove through the COMPOSED router that it is mounted and is NOT behind the CSRF
    /// layer (its callers hold a pod token, not a cookie).
    pub fn with_apps_pod(mut self, apps_pod: egret_apps::PodApiState) -> Self {
        self.apps_pod = Some(apps_pod);
        self
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
    doc.merge(egret_automation::openapi());
    doc.merge(egret_auth::openapi());
    doc.merge(egret_workspace::openapi());
    doc.merge(egret_notebook::openapi());
    doc.merge(egret_kernel::openapi());
    doc.merge(egret_controlplane::openapi());
    doc.merge(egret_catalog_engine::openapi());
    doc.merge(egret_policy::openapi());
    doc.merge(egret_perm_compile::openapi());
    doc.merge(egret_engine_mgmt::openapi());
    doc.merge(egret_machine_idp::openapi());
    doc.merge(egret_dataapi::openapi());
    doc.merge(egret_workflow::openapi());
    doc.merge(egret_apps::openapi());
    // A SECOND document from the same crate, kept as its own line rather than
    // folded into the one above: the management surface is a browser-session API
    // under `/api/ws/{workspace}/…` and the serving surface is a machine API
    // under `/data-api/v1/{ws}/…`, with different auth and a different audience.
    doc.merge(egret_dataapi::serving_openapi());
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
        // Prometheus scrape. Beside the health routes and for the same reasons:
        // unauthenticated, outside every CSRF/auth layer, and — like them —
        // carrying no `#[utoipa::path]`, so it is in no seam document and the
        // committed `openapi.json` is unchanged by its existence. See
        // `crate::metrics` for why that is the right precedent and for what the
        // exposure rests on.
        .route("/metrics", get(metrics::metrics_scrape))
        // OpenAPI spec: no auth/CSRF so FE codegen + tooling can fetch it.
        .route("/api-docs/openapi.json", get(openapi_json));

    if let Some(auth) = state.auth.clone() {
        router = router.merge(
            Router::new()
                .nest("/api/auth", auth_router())
                // Platform-admin impersonation start (POST /api/admin/impersonate);
                // exit lives on the auth protected router (/api/auth/impersonation/exit).
                .nest(
                    "/api/admin",
                    egret_auth::routes::admin_impersonation_router(),
                )
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

    if let Some(automation) = state.automation.clone() {
        let group = Router::new()
            .nest("/api", egret_automation::automation_router())
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(automation.clone());
        router = router.merge(group);

        // The renderer's endpoints, WITHOUT the CSRF layer — they carry a
        // bearer token rather than a cookie, so there is no ambient credential
        // for CSRF to protect, and the headless browser has no cookie to echo.
        // See `render_router`'s own comment for what shipping this inside the
        // layered group actually did.
        let render = Router::new()
            .nest("/api", egret_automation::render_router())
            .with_state(automation);
        router = router.merge(render);
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

    if let Some(apps) = state.apps.clone() {
        // Data Apps gateway: bearer-token (per marimo session) callers inside app pods; no
        // cookie is involved, so no CSRF layer (spec §4.2). Gated for the `apps` feature inside the handler.
        let gateway_group = Router::new()
            .nest("/api", egret_apps::gateway_router())
            .with_state(apps);
        router = router.merge(gateway_group);
    }

    if let Some(apps_pod) = state.apps_pod.clone() {
        // Data Apps pod API: bearer pod token, called by marimo pods; no cookie, so no
        // CSRF layer (spec §5.3). Authorization is re-checked inside every handler.
        let pod_group = Router::new()
            .nest("/api", egret_apps::pod_router())
            .with_state(apps_pod);
        router = router.merge(pod_group);
    }

    if let Some(apps_api) = state.apps_api.clone() {
        // Data Apps management (CRUD, drafts, publish, versions): browser-session
        // routes, CSRF-protected like the notebook group.
        let group = Router::new()
            .nest("/api", egret_apps::apps_router())
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(apps_api);
        router = router.merge(group);
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

    if let Some(pm) = state.perm.clone() {
        // The data-permission model (SP4a Task 9). Same /api/control-plane
        // prefix and CSRF layer as the policy console it will eventually
        // replace; gated on the SAME `policy:read`/`policy:update` keys.
        let group = Router::new()
            .nest("/api/control-plane", egret_perm_compile::perm_router())
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(pm);
        router = router.merge(group);
    }

    if let Some(em) = state.engine_mgmt.clone() {
        let group = Router::new()
            .nest(
                "/api/control-plane",
                egret_engine_mgmt::engine_mgmt_router(),
            )
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(em.clone());
        router = router.merge(group);

        // Workspace-admin engine-assignment endpoints (P1), nested under
        // /api/admin so the full paths are /api/admin/workspaces/{id}/engines.
        let ws_group = Router::new()
            .nest("/api/admin", egret_engine_mgmt::workspace_engines_router())
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(em);
        router = router.merge(ws_group);
    }

    if let Some(dataapi) = state.dataapi.clone() {
        // Data API management (P2): workspace-scoped, session-authenticated,
        // browser-facing — so it sits inside the CSRF layer like every other
        // `/api/ws/…` group.
        let group = Router::new()
            .nest("/api", egret_dataapi::management_router())
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(dataapi.clone());
        router = router.merge(group);

        // The serving surface (P4), WITHOUT the CSRF layer and outside `/api` —
        // the same reasoning `render_router` above carries. Double-submit CSRF
        // defends requests a browser authenticates AMBIENTLY; nothing is ambient
        // here, because the caller sets an `Authorization` header itself and a
        // cross-site page cannot. Requiring a CSRF cookie an external client can
        // never have would simply make every POST endpoint impossible — which is
        // exactly what shipping the renderer inside the layered group did, and
        // no router-level test could see it because the integration tests called
        // that router directly. `tests/dataapi_serving_mount.rs` is the test that
        // can: it goes through THIS composed router, which is also why
        // `AppState::with_dataapi` exists.
        let serving = Router::new()
            .nest("/data-api/v1", egret_dataapi::serving_router())
            .with_state(dataapi);
        router = router.merge(serving);
    }

    if let Some(workflow) = state.workflow.clone() {
        // Workflow management (W1 Task 7): workspace-scoped, session-
        // authenticated, browser-facing — same CSRF placement as `dataapi`
        // just above and for the identical reason.
        let group = Router::new()
            .nest("/api", egret_workflow::management_router())
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(workflow.clone());
        router = router.merge(group);

        // The webhook surface (W6 Task 7), WITHOUT the CSRF layer and outside
        // `/api` — the same placement, and the same two reasons, as the Data
        // API serving group above.
        //
        // Outside `/api`: `WsContext`'s feature-entitlement gate recognises a
        // path beginning `/api/ws/` or `/ws/`, and this caller has no session
        // for it to resolve. Outside CSRF: a webhook caller is a machine that
        // types the credential into the URL, so nothing is ambient and a CSRF
        // cookie it can never hold would make every POST impossible — which is
        // exactly what shipping `render_router` inside the layered group did.
        //
        // **It is anonymous on purpose, and therefore OFF by default.** The key
        // in the path is the whole credential;
        // `egret_workflow::webhook_routes`' module doc is where the scoping
        // argument lives, and its security review's verdict is "not approved
        // for exposure".
        //
        // `AppState::workflow_webhooks_enabled` is the lever that verdict
        // needs. Before fix round 2 this `nest` was unconditional, so the
        // warning had nothing behind it — an operator who agreed with it could
        // not act on it. When the flag is false the routes are **not mounted**,
        // so the paths 404 like any other unrouted URL rather than being
        // mounted-and-refusing, and nothing about them is discoverable.
        if state.workflow_webhooks_enabled {
            let webhooks = Router::new()
                .nest("/workflows/v1", egret_workflow::webhook_router())
                .with_state(workflow);
            router = router.merge(webhooks);
        }
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

    // Outermost, so it runs before every route and every other layer: a request a
    // sibling host (the apps origin) made with the viewer's cookies never reaches a
    // handler. See `egret_auth::fetch_meta`.
    router.with_state(state).layer(axum::middleware::from_fn(
        egret_auth::fetch_meta::refuse_same_site,
    ))
}

/// Liveness: the process is up. Always 200.
async fn healthz() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

/// How long `/readyz` waits for the database before answering `503` itself.
///
/// # The probe had no bound of its own, and the pool's is 30 s
///
/// `Db::connect` builds the pool with sqlx's default `acquire_timeout` of **30
/// seconds**. Without a bound here, a `/readyz` that cannot get a connection
/// waits all 30 of them — long past the kubelet's `timeoutSeconds`, which means
/// the kubelet abandons the probe and the abandoned `acquire` stays queued
/// behind the traffic that was already there. Under the load that causes it,
/// that is one more waiter every `periodSeconds`.
///
/// # Bulkhead width, pool size and probe timings are ONE set of numbers
///
/// They were chosen in three different places and only make sense read together,
/// so they are written down here:
///
/// | number | value | where |
/// |---|---|---|
/// | serving bulkhead | 64 in flight | `SERVING_MAX_CONCURRENCY` |
/// | trial bulkhead | 8 in flight | `egret_dataapi::TRIAL_MAX_CONCURRENCY` |
/// | metastore pool | 10 connections | `egret_config`'s `database.max_connections` |
/// | pool acquire timeout | 30 s | sqlx's default, via `Db::connect` |
/// | request deadline | 50 s | `SERVING_REQUEST_DEADLINE` |
/// | readiness probe | 3 s × 5 s × N | `backend-deployment.yaml` |
///
/// The bulkhead is deliberately **8× the pool** — see its own doc: it is a wall
/// against the Data API consuming the process, not a throughput limit, and the
/// pool is meant to be the thing that paces ordinary work. The consequence is
/// that **a pool with no free connection is the normal busy state of a healthy
/// pod**, not a fault. Two things follow, and both are the reconciliation:
///
/// 1. **`/readyz` must answer before the kubelet gives up**, so this bound sits
///    below the probe's `timeoutSeconds: 3`. An answer egret writes is one the
///    operator can read; a probe the kubelet abandons is a timeout with no
///    explanation and a queued waiter left behind.
/// 2. **Withdrawal must take longer than one request.** One admitted request may
///    legitimately hold its connection for `SERVING_REQUEST_DEADLINE`, so a
///    readiness budget shorter than that is guaranteed to withdraw pods that are
///    merely busy — and every withdrawal moves that load onto the peers, which
///    then fail the same check. `periodSeconds × failureThreshold` is therefore
///    set above 50 s in the chart, and `deployment_probes.rs` asserts both
///    relations against these constants rather than against a comment.
///
/// The residual, stated: a genuinely dead database keeps receiving traffic for
/// the whole withdrawal budget. Those requests fail fast and loudly with a 500
/// each; a healthy pod withdrawn under load fails an entire deployment quietly.
pub const READYZ_DB_TIMEOUT: Duration = Duration::from_secs(2);

/// Readiness: dependencies are reachable. 200 when the DB (if any) responds.
async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    match &state.db {
        None => (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "ok", "db": "none" })),
        ),
        // `timeout` CANCELS the ping, which is the half that matters: the
        // dropped future takes its place in the pool's acquire queue with it.
        Some(db) => match tokio::time::timeout(READYZ_DB_TIMEOUT, db.ping()).await {
            Ok(Ok(())) => (
                StatusCode::OK,
                Json(serde_json::json!({ "status": "ok", "db": "up" })),
            ),
            Ok(Err(_)) => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "status": "degraded", "db": "down" })),
            ),
            // Distinguished in the body, not in the status: for the kubelet this
            // is the same "not ready", but for the operator reading a probe
            // response by hand "slow" and "refused" are different incidents.
            Err(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "status": "degraded", "db": "slow" })),
            ),
        },
    }
}

/// How long the process keeps serving after SIGTERM before it stops waiting.
///
/// # Why there is a deadline at all
///
/// `axum::serve(..).with_graceful_shutdown(..)` waits for every in-flight
/// request with no bound of its own. A single request stuck on an unresponsive
/// engine would hold the whole shutdown open until the kubelet's SIGKILL — which
/// is the same outcome as no drain at all, arrived at slowly. So the drain is
/// bounded, and the bound is stated where the numbers it sits between can be
/// read together.
///
/// # The three numbers, and the order they must be in
///
/// ```text
/// egret_dataapi::serving::SERVING_REQUEST_DEADLINE   50 s
/// DRAIN_DEADLINE                                     55 s
/// terminationGracePeriodSeconds (Helm)               65 s
/// ```
///
/// * **> the request deadline**, so a request admitted one moment before SIGTERM
///   gets its full budget and is answered rather than cut in half. At 50 s the
///   drain would race the very deadline that is supposed to end that request,
///   and the caller would sometimes see a connection reset instead of a `504`.
/// * **< the pod's termination grace period**, so the process is the thing that
///   decides to stop, not SIGKILL. Kubernetes' default grace period is **30 s**,
///   which is *below* the request deadline: leaving it at the default would have
///   made this whole drain unreachable, so `backend-deployment.yaml` sets 65 and
///   `the_drain_deadline_fits_inside_the_pods_termination_grace_period` pins the two
///   together.
///
/// # Why the drain matters more here than the numbers suggest
///
/// `egret_dataapi::serving::router::serve` writes its `dataapi.access_log` row
/// **after** the response. A request killed mid-flight is therefore not merely
/// failed, it is unrecorded: the operator sees a consumer complaining about an
/// error that appears in no activity view. Every deploy did this to every
/// in-flight query before this change.
pub const DRAIN_DEADLINE: Duration = Duration::from_secs(55);

/// Resolve when the process is asked to stop: SIGTERM (what Kubernetes sends) or
/// SIGINT (what a developer's Ctrl-C sends).
///
/// SIGTERM is the one that matters and the one that was missing — nothing in the
/// process observed it, so a rollout terminated every in-flight query.
pub async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                // Losing the handler must not lose the drain: fall back to
                // Ctrl-C alone rather than returning immediately, which would
                // shut the server down the instant it started.
                tracing::error!(
                    error = %e,
                    "cannot listen for SIGTERM; drain will only honour Ctrl-C"
                );
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = term.recv() => tracing::info!("SIGTERM received; draining"),
            _ = tokio::signal::ctrl_c() => tracing::info!("SIGINT received; draining"),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("Ctrl-C received; draining");
    }
}

/// Serve `app` until `shutdown` resolves, then finish in-flight requests for at
/// most `drain_deadline`.
///
/// `shutdown` and `drain_deadline` are parameters rather than reads of
/// [`shutdown_signal`] and [`DRAIN_DEADLINE`] for one reason: a test must be
/// able to trigger the drain and observe that an **in-flight request completed**
/// — not that a handler was installed. A test that asserted installation would
/// pass with the drain removed, which is the failure this repo has shipped
/// repeatedly.
pub async fn serve_with_drain<F>(
    listener: tokio::net::TcpListener,
    app: Router,
    shutdown: F,
    drain_deadline: Duration,
) -> std::io::Result<()>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    // The signal is observed once and then broadcast twice: `axum::serve` needs
    // it to stop accepting, and the deadline below needs it to start counting.
    // Starting the timer any earlier would make it a bound on UPTIME.
    let (draining_tx, mut draining_rx) = tokio::sync::watch::channel(false);
    let server = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown.await;
            let _ = draining_tx.send(true);
        })
        // `WithGracefulShutdown` is `IntoFuture`, not `Future`, so it cannot be
        // polled by `select!` until it is converted.
        .into_future();
    tokio::pin!(server);

    tokio::select! {
        res = &mut server => res,
        () = async {
            // `changed()` resolves on the send above, never on the initial
            // `false` — so the sleep is entered at SIGTERM and not at boot.
            let _ = draining_rx.changed().await;
            tokio::time::sleep(drain_deadline).await;
        } => {
            tracing::warn!(
                deadline_secs = drain_deadline.as_secs(),
                "requests still in flight at the drain deadline; exiting anyway"
            );
            Ok(())
        }
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

    /// The half of `/readyz` that matters now that a probe actually calls it.
    ///
    /// `readyz_ok_without_db` above pins the `db: None` path, which is the one
    /// that cannot fail — it would pass with the `ping()` deleted. This drives a
    /// pool pointed at a port nothing listens on and requires the `503`, so the
    /// handler is shown to have a *reachable* failing answer. Without that, the
    /// chart change in this package would wire a probe to an endpoint that is
    /// green whatever the database is doing, which is where it started.
    #[tokio::test]
    async fn readyz_reports_503_when_the_database_is_unreachable() {
        // Port 1: nothing listens, so the ping cannot succeed. The short
        // `acquire_timeout` is the whole reason this builds the pool by hand —
        // `Db::connect_lazy` inherits sqlx's 30 s default, and a 30-second unit
        // test is one nobody runs.
        let pool = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(Duration::from_millis(250))
            .connect_lazy("postgres://egret:egret@127.0.0.1:1/egret")
            .expect("lazy pool");
        let mut state = AppState::without_db();
        state.db = Some(Db { pool });
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["db"], "down");
    }

    /// The readiness probe must not inherit the pool's 30-second patience.
    ///
    /// A refused connection answers instantly and is the easy case; the one that
    /// matters is a database that **accepts and then says nothing** — a failing
    /// over primary, a blackholed route, a saturated pool. This binds a socket
    /// and never accepts on it, so the client completes the TCP handshake in the
    /// kernel's backlog and then blocks forever waiting for the Postgres
    /// greeting. The pool is built with sqlx's real default `acquire_timeout`,
    /// so with the bound deleted this test does not fail fast — it waits the
    /// full 30 s and then reports `db: "down"`, which is both slower than the
    /// kubelet's `timeoutSeconds` and the wrong diagnosis.
    #[tokio::test]
    async fn readyz_gives_up_on_a_hung_database_long_before_the_pool_does() {
        // Never accepted. Held for the duration of the test so the port stays
        // bound and the connection stays parked rather than being refused.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind blackhole");
        let port = listener.local_addr().expect("addr").port();

        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy(&format!("postgres://egret:egret@127.0.0.1:{port}/egret"))
            .expect("lazy pool");
        let mut state = AppState::without_db();
        state.db = Some(Db { pool });
        let app = build_router(state);

        let started = std::time::Instant::now();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let elapsed = started.elapsed();

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(
            elapsed < READYZ_DB_TIMEOUT * 3,
            "/readyz took {elapsed:?} against a hung database. Its own bound is \
             {READYZ_DB_TIMEOUT:?}; anything near sqlx's 30 s acquire_timeout \
             means the probe has no bound of its own and the kubelet is deciding \
             it instead"
        );

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            json["db"], "slow",
            "a database that hangs is a different incident from one that refuses, \
             and the body is the only place an operator can tell them apart: {json}"
        );
        drop(listener);
    }

    /// A `Pdp` that must never actually be consulted in this test: every
    /// request below carries no session cookie, so `Auth` rejects it (401)
    /// before any authz check runs. It exists purely to satisfy
    /// `WorkspaceState`'s type — a `CedarPdp` would need to query a live
    /// database at build time, which this test deliberately has none of.
    struct UnreachablePdp;
    #[async_trait::async_trait]
    impl egret_core::authz::Pdp for UnreachablePdp {
        async fn authorize(
            &self,
            _subject: &egret_core::authz::Subject,
            _action: &str,
            _resource: &egret_core::authz::Resource,
        ) -> egret_core::Result<egret_core::authz::Decision> {
            unreachable!("no request in this test should reach an authz check")
        }
    }

    /// A `WorkspaceState` built without a live database: `Db::connect_lazy`
    /// only parses the URL and never opens a connection, so this is safe to
    /// build in a unit test with no Postgres running. Every repo/service here
    /// just wraps the `Db` handle at construction — none of it queries — so
    /// the only way this could touch the network is if a handler under test
    /// actually ran a DB call, which the mount-reachability checks below never
    /// do (they 401 on the missing session cookie first).
    fn workspace_state_for_mount_check() -> WorkspaceState {
        let db = Db::connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
            .expect("connect_lazy only parses the URL; it never touches the network");
        WorkspaceState {
            workspaces: egret_workspace::workspaces::WorkspaceRepo::new(db.clone()),
            members: egret_workspace::members::MemberRepo::new(db.clone()),
            access_views: egret_workspace::access_views::AccessViewsRepo::new(db.clone()),
            access_requests: egret_workspace::access_requests::AccessRequestRepo::new(
                db.clone(),
                egret_workspace::members::MemberRepo::new(db.clone()),
            ),
            authz: egret_workspace::rbac::Authorizer::new(db.clone()),
            pdp: std::sync::Arc::new(UnreachablePdp),
            users: egret_auth::users::UserRepo::new(db.clone()),
            sessions: egret_auth::session::SessionService::new(
                db.clone(),
                chrono::Duration::hours(24),
                chrono::Duration::minutes(60),
            ),
            audit: egret_audit::AuditSink::new(16).0,
            impersonations: egret_auth::impersonation::ImpersonationRepo::new(db.clone()),
            oidc: None,
            cookie_key: None,
            session_secure: false,
        }
    }

    /// Structural guard against a mount-path regression: `features_routes.rs`
    /// (in `egret-workspace`) builds its own ad hoc router nested at
    /// `/admin`/`/me`, so it would keep passing even if the *real* mount in
    /// `build_router` above ever drifted from `/api/admin`/`/api/me` — and
    /// `committed_openapi_json_is_current` wouldn't catch that either, since it
    /// only compares the static `#[utoipa::path(path = …)]` string, never the
    /// actual mount. This builds the real router and asserts the features
    /// routes are genuinely reachable at their production path.
    ///
    /// The assertion is "not 404" rather than a specific status: the point is
    /// reachability of the path, not the auth outcome. These requests carry no
    /// session cookie, so today they 401 — but pinning that exact status would
    /// couple this test to authentication behaviour it isn't about; a 404
    /// specifically means "not mounted here", which is the one thing this test
    /// exists to catch.
    #[tokio::test]
    async fn workspace_feature_routes_are_mounted_at_their_documented_path() {
        let mut state = AppState::without_db();
        state.workspace = Some(workspace_state_for_mount_check());
        let app = build_router(state);

        let some_uuid = "00000000-0000-0000-0000-000000000000";
        for uri in [
            format!("/api/admin/workspaces/{some_uuid}/features"),
            "/api/me/workspaces".to_string(),
        ] {
            let resp = app
                .clone()
                .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_ne!(
                resp.status(),
                StatusCode::NOT_FOUND,
                "{uri} must be mounted (got 404 — not authentication, which would be 401/403)"
            );
        }
    }
}
