//! Build lakekeeper's router state in-process and mount its Iceberg REST
//! router as an [`axum::Router`] — without binding a TCP port.
//!
//! This is the headless ("embedded") counterpart to `lakekeeper-bin`'s
//! `serve.rs`. The binary builds the same `ApiContext<State<A, C, S>>` and
//! then hands it to `lakekeeper::serve::serve`, which binds a socket and spins
//! up background services (task-queue workers, endpoint-stats tracker, cloud
//! events, health checks, metrics server). For the spike we want none of that
//! plumbing — we just want the **router + state** so Task 4 can drive it with
//! a `tower::ServiceExt::oneshot` (no network).
//!
//! ## The state type lakekeeper's router needs (KEY FINDING)
//!
//! `lakekeeper::api::iceberg::v1::new_v1_full_router::<T, S>()` returns
//! `Router<ApiContext<S>>` where:
//!
//! * `S: lakekeeper::api::ThreadSafe` — the *state* type. The concrete state
//!   lakekeeper uses (see `api/router.rs::new_full_router`) is
//!   `lakekeeper::service::State<A, C, S>` where `A: Authorizer + Clone` (we use
//!   [`EgretAuthorizer`]), `C: CatalogStore` (we use `PostgresBackend`), and
//!   `S: SecretStore` (we use postgres `SecretsState`). `State<A, C, S>` itself
//!   `impl ThreadSafe` (it is `Clone + Send + Sync + 'static`).
//! * `T` — the *service implementation* that satisfies the catalog endpoint
//!   traits. lakekeeper passes `lakekeeper::server::CatalogServer<C, A, S>`.
//!   `T` must satisfy ALL of (with `s3-signer` on by default via the `router`
//!   feature): `config::Service<S>`, `namespace::NamespaceService<S>`,
//!   `tables::TablesService<S>`, `metrics::Service<S>`,
//!   `s3_signer::Service<S>`, `views::ViewService<S>`. `CatalogServer<C, A, S>`
//!   blanket-implements all of these (it is the upstream impl), so we never
//!   name those traits directly — we just pass the concrete `CatalogServer`.
//!
//! ## The `State` struct fields and how we fill them
//!
//! `State { authz, catalog, secrets, contract_verifiers, events,
//! registered_task_queues, license_status, build_info }`:
//!
//! * `authz` → `EgretAuthorizer { server_id, system_principal }`
//! * `catalog` → `CatalogState::from_pools(read, write)` over our isolated
//!   `lakekeeper`-schema pools.
//! * `secrets` → postgres `SecretsState::from_pools(read, write)`.
//! * `contract_verifiers` → `ContractVerifiers::default()` (no verifiers).
//! * `events` → `EventDispatcher::new(vec![])` (no listeners).
//! * `registered_task_queues` → populated by `register_built_in_queues` on the
//!   returned `TaskQueueRegistry` (tabular-expiry + purge + task-log cleanup).
//!   The router reads this handle via `api_config()`. Workers are NOT spawned
//!   here — the binary calls [`spawn_gc_workers`] after `build_state` (CE-6).
//! * `license_status` → `&APACHE_LICENSE_STATUS` (the OSS default).
//! * `build_info` → `&DEFAULT_BUILD_INFO` (empty build info).
//!
//! `server_id` for the authorizer is read back from the catalog via
//! `PostgresBackend::get_server_info`, exactly as `serve.rs` does.

use anyhow::Context as _;
use lakekeeper::api::iceberg::v1::new_v1_full_router;
use lakekeeper::api::management::v1::server::{APACHE_LICENSE_STATUS, DEFAULT_BUILD_INFO};
use lakekeeper::api::ApiContext;
use lakekeeper::api::RequestMetadata;
use lakekeeper::server::CatalogServer;
use lakekeeper::service::authz::AllowAllAuthorizer;
use lakekeeper::service::contract_verification::ContractVerifiers;
use lakekeeper::service::events::EventDispatcher;
use lakekeeper::service::tasks::TaskQueueRegistry;
use lakekeeper::service::{CatalogStore, State};
use lakekeeper_storage_postgres::{CatalogState, PostgresBackend, SecretsState};

use crate::pool::build_isolated_pool;

/// The concrete state type lakekeeper's router is generic over.
///
/// `A = AllowAllAuthorizer` (EB-1: lakekeeper's permissive built-in authorizer;
/// EB-3 swaps in the real egret authorizer), `C = PostgresBackend`,
/// `S = SecretsState`. `ApiContext<EngineState>` is what `new_v1_full_router`
/// returns a `Router<_>` for, and the value callers `.with_state(...)`.
pub type EngineState = State<AllowAllAuthorizer, PostgresBackend, SecretsState>;

/// The concrete service implementation that satisfies the iceberg endpoint
/// traits the router requires (`config`/`namespace`/`tables`/`metrics`/
/// `s3_signer`/`views` `Service`s). Upstream blanket-implements all of them.
pub type EngineServer = CatalogServer<PostgresBackend, AllowAllAuthorizer, SecretsState>;

/// Build the lakekeeper [`ApiContext`] over isolated `lakekeeper`-schema pools,
/// and return the populated [`TaskQueueRegistry`] for the caller to spawn workers.
///
/// Ports `lakekeeper-bin/src/serve.rs::get_default_catalog_from_config` +
/// `serve_inner`'s state assembly, but: it uses our [`build_isolated_pool`]
/// (search_path-pinned) instead of lakekeeper's config-driven pools, and stops
/// at the `ApiContext` — no background services, no socket.
///
/// The isolated pool sets `search_path = lakekeeper, public`; we build two
/// (read + write) so `CatalogState`/`SecretsState` get the distinct pools their
/// `from_pools` constructors expect (we point both at the same DSN — the spike
/// has no read replica).
///
/// `system_user` (the configured `catalog.system_user`) is accepted for a stable
/// public signature; EB-1's [`AllowAllAuthorizer`] needs no per-principal bypass,
/// so it is unused here. [`crate::bootstrap::ensure_bootstrapped`] still consumes
/// it to derive the stable admin principal.
///
/// The returned [`TaskQueueRegistry`] has the 3 built-in GC queues registered
/// (tabular expiry + purge + task-log cleanup). The binary passes it to
/// [`spawn_gc_workers`] to start the workers (CE-6).
///
/// # Errors
/// Returns an error if the pools cannot be built, the server info (server id)
/// cannot be read back from the catalog, or the system principal id cannot be
/// derived.
pub async fn build_state(
    dsn: &str,
    system_user: &str,
) -> anyhow::Result<(ApiContext<EngineState>, TaskQueueRegistry)> {
    // Prod poll interval: 10s between task-queue polls (the same cadence
    // lakekeeper-bin's serve.rs uses). Tests that need a faster pickup use
    // `build_state_with_gc_poll` (test-utils-gated).
    build_state_with_gc_poll_inner(dsn, system_user, std::time::Duration::from_secs(10)).await
}

/// A test-only variant of [`build_state`] that takes the task-queue **poll
/// interval** explicitly, so an integration test can register the built-in GC
/// queues with a short (e.g. 1s) cadence and not wait the prod 10s per tick.
///
/// Identical to [`build_state`] in every other respect — same isolated pools,
/// same authorizer, same `State` assembly, and it registers the SAME 3 built-in
/// queues into the returned [`TaskQueueRegistry`]. The only difference is the
/// `poll` passed to `register_built_in_queues`. Gated behind the crate's
/// `test-utils` feature so the prod build never exposes a tunable poll knob and
/// [`build_state`]'s public signature is unchanged.
///
/// # Errors
/// Same as [`build_state`].
#[cfg(feature = "test-utils")]
pub async fn build_state_with_gc_poll(
    dsn: &str,
    system_user: &str,
    poll: std::time::Duration,
) -> anyhow::Result<(ApiContext<EngineState>, TaskQueueRegistry)> {
    build_state_with_gc_poll_inner(dsn, system_user, poll).await
}

/// Shared body of [`build_state`] / [`build_state_with_gc_poll`]. Private so the
/// prod path keeps its no-argument `poll` baked in.
async fn build_state_with_gc_poll_inner(
    dsn: &str,
    _system_user: &str,
    poll: std::time::Duration,
) -> anyhow::Result<(ApiContext<EngineState>, TaskQueueRegistry)> {
    // Two isolated pools (read + write). Both pin search_path = lakekeeper,public.
    let read_pool = build_isolated_pool(dsn)
        .await
        .context("building isolated read pool")?;
    let write_pool = build_isolated_pool(dsn)
        .await
        .context("building isolated write pool")?;

    let catalog: CatalogState = CatalogState::from_pools(read_pool.clone(), write_pool.clone());
    let secrets: SecretsState = SecretsState::from_pools(read_pool, write_pool);

    // server_id is required to construct the authorizer. Read it back from the
    // (already-migrated) catalog, exactly like serve.rs does.
    let server_id = <PostgresBackend as CatalogStore>::get_server_info(catalog.clone())
        .await
        .context("reading lakekeeper server info (catalog must be migrated first)")?
        .server_id();
    // EB-1: lakekeeper's permissive built-in authorizer. EB-3 replaces it with
    // the real egret authorizer that gates data actions per-token.
    let authz = AllowAllAuthorizer { server_id };

    // Register the 3 built-in GC queues (tabular expiry + purge + task-log
    // cleanup). We clone catalog/secrets/authz here so the originals still move
    // into State below. Workers are NOT spawned here — the binary calls
    // spawn_gc_workers with the returned registry (CE-6).
    let registry = TaskQueueRegistry::new();
    registry
        .register_built_in_queues::<PostgresBackend, SecretsState, AllowAllAuthorizer>(
            catalog.clone(),
            secrets.clone(),
            authz.clone(),
            poll,
        )
        .await;
    let registered_task_queues = registry.registered_task_queues();

    let state = State {
        authz,
        catalog,
        secrets,
        contract_verifiers: ContractVerifiers::default(),
        events: EventDispatcher::new(vec![]),
        registered_task_queues,
        license_status: &APACHE_LICENSE_STATUS,
        build_info: &DEFAULT_BUILD_INFO,
    };

    Ok((ApiContext { v1_state: state }, registry))
}

/// Spawn lakekeeper's built-in GC task-queue workers (tabular expiration +
/// purge + task-log cleanup). Runs until `cancel` fires; restarts any worker
/// that exits. Call once, after [`build_state`], from the binary.
pub fn spawn_gc_workers(registry: TaskQueueRegistry, cancel: lakekeeper::CancellationToken) {
    tokio::spawn(async move {
        let runner = registry.task_queues_runner(cancel).await;
        runner.run_queue_workers(true).await;
    });
}

/// Build the lakekeeper Iceberg REST v1 router, with the engine state applied.
///
/// Mirrors `api/router.rs::new_full_router`'s core call —
/// `new_v1_full_router::<CatalogServer<C, A, S>, State<A, C, S>>()` — but
/// returns the catalog routes only (no `/management`, no auth/cors/trace
/// middleware stack, no health route). For the spike that is exactly what we
/// want: a `Router` carrying the Iceberg catalog endpoints that Task 4 can
/// `oneshot`. The upstream router nests these under `/catalog/v1`; callers who
/// want that prefix can `Router::new().nest("/catalog/v1", router)`.
///
/// `new_v1_full_router` returns `Router<ApiContext<EngineState>>`; we apply the
/// state with `.with_state(...)` so the result is a ready-to-serve
/// `axum::Router`.
pub fn catalog_engine_router(state: ApiContext<EngineState>) -> axum::Router {
    new_v1_full_router::<EngineServer, EngineState>().with_state(state)
}

/// Assemble the catalog engine router: the lakekeeper Iceberg REST router built
/// by [`catalog_engine_router`].
///
/// EB-1 mounts the bare engine — the router carries lakekeeper's own
/// [`AllowAllAuthorizer`], with no egret front-of-router auth / token-ownership
/// middleware (that legacy glue depended on egret-catalog's deleted warehouse +
/// token store). EB-3 reintroduces egret's authn/authz layer here.
///
/// The returned router is lakekeeper-relative — its routes are `/{prefix}/...`
/// (catalog ops) and `/config` (warehouse discovery). The composition root nests
/// it under `/catalog/v1`.
///
/// * `engine_state` — the lakekeeper [`ApiContext`] from [`build_state`]
///   (migrated + bootstrapped).
/// egret's authN + authZ guard for embedded-catalog requests (EB-3). Implemented
/// in the composition root (egret-bin) over the mini-IdP (JWT validation) + the
/// RBAC store (warehouse/role → grants). Keeping this in egret owns the decision
/// while lakekeeper runs with its permissive [`AllowAllAuthorizer`].
///
/// `path`/`query` are lakekeeper-relative (the router is mounted under
/// `/catalog/v1`, so a request to `/catalog/v1/config` arrives here as `/config`).
///
/// `body` is the request body for a rename or a multi-table commit
/// ([`GuardedBody`]), whose targets are named only there, and `None` for every
/// other request, which streams through unread.
#[async_trait::async_trait]
pub trait CatalogGuard: Send + Sync {
    /// `Ok(caller)` → allowed, by the principal the verified token names;
    /// `Err((status, message))` → rejected (e.g. 400/401/403).
    async fn check(
        &self,
        method: &str,
        path: &str,
        query: Option<&str>,
        bearer: Option<&str>,
        body: Option<&[u8]>,
    ) -> Result<CatalogCaller, (axum::http::StatusCode, String)>;
}

/// The requests that name their targets only in the body, whose body the
/// mount hands the guard (rename-authz spec §3.1, §8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardedBody {
    /// `POST /{prefix}/tables/rename` or `POST /{prefix}/views/rename`.
    Rename,
    /// `POST /{prefix}/transactions/commit`.
    MultiTableCommit,
}

impl GuardedBody {
    /// Which of them this request is, if any.
    pub fn of(method: &str, path: &str) -> Option<Self> {
        if method != "POST" {
            return None;
        }
        let segs: Vec<&str> = path
            .trim_start_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();
        match segs.as_slice() {
            [_, "tables" | "views", "rename"] => Some(Self::Rename),
            [_, "transactions", "commit"] => Some(Self::MultiTableCommit),
            _ => None,
        }
    }

    /// The most of the body the mount buffers. A rename body is a few hundred
    /// bytes. A multi-table commit carries whole schemas and snapshots; the
    /// table hook behind the guard already buffers it up to 16 MiB, so the
    /// guard takes the same cap and adds no new limit.
    pub fn limit(self) -> usize {
        match self {
            Self::Rename => 1 << 20,
            Self::MultiTableCommit => 16 << 20,
        }
    }

    /// How the guard's 400 names the request.
    pub fn what(self) -> &'static str {
        match self {
            Self::Rename => "rename request",
            Self::MultiTableCommit => "multi-table commit",
        }
    }
}

/// Who sent an allowed Iceberg request: the verified token's `sub` and `aud`.
/// [`CatalogGuard::check`] returns it and the mount puts it in the request's
/// extensions, so a layer behind the guard can name the caller (the table
/// hook's governance events, rename spec R7). Today every catalog token is
/// minted for a catalog client (`sub` = its `client_id`, `aud = "catalog"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogCaller {
    pub subject: String,
    pub audience: String,
}

/// Assemble the catalog engine router, fronted by egret's [`CatalogGuard`] (when
/// `Some`), then a system [`RequestMetadata`] for lakekeeper's handlers, then
/// the view-governance hold (when `Some`).
///
/// * `engine_state` — the lakekeeper [`ApiContext`] from [`build_state`].
/// * `guard` — egret's authN/authZ decision. `None` (tests) → no gate, every
///   request runs as the internal system actor under `AllowAllAuthorizer`.
/// * `hold` — the view hold ([`crate::view_hold`], spec B2). It runs after the
///   guard, so an unauthenticated request does no lineage work, and before
///   lakekeeper, so a classified view is not committed until its tags are on
///   every plugin.
/// * `table_hook` — the table-commit hook ([`crate::table_hook`], D2): it
///   recompiles the bound StarRocks profiles when a table classified as a
///   whole changes schema. It sits inside the guard and the view hold, next to
///   lakekeeper.
pub fn mount_catalog_engine(
    engine_state: ApiContext<EngineState>,
    guard: Option<std::sync::Arc<dyn CatalogGuard>>,
    hold: Option<std::sync::Arc<crate::view_hold::ViewHold>>,
    table_hook: Option<std::sync::Arc<crate::table_hook::TableHook>>,
) -> axum::Router {
    let inner = catalog_engine_router(engine_state);
    let inner = match table_hook {
        Some(hook) => crate::table_hook::layer(inner, hook),
        None => inner,
    };
    front_catalog_router(inner, guard, hold)
}

/// The layers [`mount_catalog_engine`] puts in front of `inner`, which is
/// lakekeeper's router in production. Tests pass lakekeeper's router wrapped
/// in their own observer, to see when a request reaches lakekeeper.
pub fn front_catalog_router(
    inner: axum::Router,
    guard: Option<std::sync::Arc<dyn CatalogGuard>>,
    hold: Option<std::sync::Arc<crate::view_hold::ViewHold>>,
) -> axum::Router {
    let inner = match hold {
        // The hold reads a stored view through lakekeeper's own router, with
        // no hold in front of it.
        Some(hold) => {
            let loader = inner.clone();
            inner.layer(axum::middleware::from_fn_with_state(
                (hold, loader),
                crate::view_hold::hold_view_commits,
            ))
        }
        None => inner,
    };
    inner.layer(axum::middleware::from_fn_with_state(
        guard,
        guard_then_inject_metadata,
    ))
}

/// Run egret's guard (if configured), then inject the system [`RequestMetadata`]
/// lakekeeper's handlers require. lakekeeper itself stays on `AllowAllAuthorizer`;
/// egret has already decided allow/deny in the guard.
async fn guard_then_inject_metadata(
    axum::extract::State(guard): axum::extract::State<Option<std::sync::Arc<dyn CatalogGuard>>>,
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    let path = req.uri().path().to_string();
    // Capture how the client reached egret, to rewrite the catalog config `uri`
    // lakekeeper otherwise advertises as its hardcoded `http://localhost:8181`.
    let host = req
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let scheme = req
        .headers()
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("http")
        .to_string();

    if let Some(g) = &guard {
        let method = req.method().as_str().to_string();
        let query = req.uri().query().map(str::to_string);
        let bearer = req
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .map(str::to_string);
        // A rename and a multi-table commit name what they touch only in the
        // body: read it for the guard and put the same bytes back, so the view
        // hold, the table hook and lakekeeper see exactly what the guard
        // authorized.
        let body = if let Some(kind) = GuardedBody::of(&method, &path) {
            let (parts, body) = req.into_parts();
            let bytes = match axum::body::to_bytes(body, kind.limit()).await {
                Ok(b) => b,
                Err(e) => {
                    return (
                        axum::http::StatusCode::BAD_REQUEST,
                        format!("malformed {}: the body could not be read: {e}", kind.what()),
                    )
                        .into_response()
                }
            };
            req = axum::extract::Request::from_parts(parts, axum::body::Body::from(bytes.clone()));
            Some(bytes)
        } else {
            None
        };
        match g
            .check(
                &method,
                &path,
                query.as_deref(),
                bearer.as_deref(),
                body.as_deref(),
            )
            .await
        {
            Ok(caller) => {
                req.extensions_mut().insert(caller);
            }
            Err((status, msg)) => return (status, msg).into_response(),
        }
    }
    req.extensions_mut()
        .insert(RequestMetadata::new_lakekeeper_internal(
            uuid::Uuid::now_v7(),
        ));
    let response = next.run(req).await;

    // Rewrite the `/config` response so the advertised catalog `uri` (and S3
    // signer uri) point back at egret via the host the client actually used —
    // lakekeeper hardcodes `http://localhost:8181` in `RequestMetadata`, which no
    // external Iceberg client (Trino, pyiceberg) could reach.
    if path == "/config" && response.status().is_success() {
        if let Some(host) = host {
            return rewrite_config_host(response, &scheme, &host).await;
        }
    }
    response
}

/// Replace lakekeeper's hardcoded `http://localhost:8181` in the `/config` body
/// with `{scheme}://{host}` (how the client reached egret).
async fn rewrite_config_host(
    response: axum::response::Response,
    scheme: &str,
    host: &str,
) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    let (mut parts, body) = response.into_parts();
    let bytes = match axum::body::to_bytes(body, 1 << 20).await {
        Ok(b) => b,
        Err(_) => return axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let replaced = String::from_utf8_lossy(&bytes)
        .replace("http://localhost:8181", &format!("{scheme}://{host}"));
    // Body length changed; drop the stale Content-Length so it is re-derived.
    parts.headers.remove(axum::http::header::CONTENT_LENGTH);
    axum::response::Response::from_parts(parts, axum::body::Body::from(replaced))
}
