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
use lakekeeper::server::CatalogServer;
use lakekeeper::service::contract_verification::ContractVerifiers;
use lakekeeper::service::events::EventDispatcher;
use lakekeeper::service::tasks::TaskQueueRegistry;
use lakekeeper::service::{CatalogStore, State};
use lakekeeper_storage_postgres::{CatalogState, PostgresBackend, SecretsState};

use crate::authorizer::EgretAuthorizer;
use crate::pool::build_isolated_pool;

/// The concrete state type lakekeeper's router is generic over.
///
/// `A = EgretAuthorizer` (CE-4: enforces the per-token egret scope claim on data
/// actions), `C = PostgresBackend`, `S = SecretsState`.
/// `ApiContext<EngineState>` is what `new_v1_full_router` returns a
/// `Router<_>` for, and the value Task 4 will `.with_state(...)`.
pub type EngineState = State<EgretAuthorizer, PostgresBackend, SecretsState>;

/// The concrete service implementation that satisfies the iceberg endpoint
/// traits the router requires (`config`/`namespace`/`tables`/`metrics`/
/// `s3_signer`/`views` `Service`s). Upstream blanket-implements all of them.
pub type EngineServer = CatalogServer<PostgresBackend, EgretAuthorizer, SecretsState>;

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
/// `system_user` is the configured `catalog.system_user`; it derives the stable
/// [`crate::bootstrap::system_user_id`] principal that [`EgretAuthorizer`]
/// bypasses (so bootstrap, provisioning, and the in-process browse API are never
/// blocked by per-token scope claims).
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
    system_user: &str,
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
    // CE-4: the EgretAuthorizer gates data actions by the per-token egret scope
    // claim, bypassing only the stable system principal (bootstrap/browse).
    let authz = EgretAuthorizer {
        server_id,
        system_principal: crate::bootstrap::system_user_id(system_user)?,
    };

    // Register the 3 built-in GC queues (tabular expiry + purge + task-log
    // cleanup). We clone catalog/secrets/authz here so the originals still move
    // into State below. Workers are NOT spawned here — the binary calls
    // spawn_gc_workers with the returned registry (CE-6).
    let registry = TaskQueueRegistry::new();
    registry
        .register_built_in_queues::<PostgresBackend, SecretsState, EgretAuthorizer>(
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

/// Assemble the LIVE catalog engine router: the lakekeeper Iceberg REST router
/// (built by [`catalog_engine_router`]) fronted by egret's per-workspace token
/// ownership middleware ([`crate::front_auth::front_auth`], CE-2 T5 / A-3).
///
/// The returned router is lakekeeper-relative — its routes are `/{prefix}/...`
/// (catalog ops) and `/config` (warehouse discovery). The composition root nests
/// it under `/catalog/v1` so the middleware sees those relative paths when it
/// extracts the `{prefix}` warehouse UUID. This is the single wiring point shared
/// by `egret-bin` (prod startup) and the CE-2 integration test, so both exercise
/// identical authn/ownership behaviour.
///
/// * `engine_state` — the lakekeeper [`ApiContext`] from [`build_state`]
///   (migrated + bootstrapped).
/// * `store` — egret-side [`egret_catalog::service::CatalogStore`] resolving
///   `token → workspace → warehouse`.
/// * `catalog_jwt` — optional OIDC-JWT authenticator (F2-P2) for the hybrid
///   JWT-or-opaque path. `None` disables the JWT path (opaque tokens only). The
///   verifier is built IdP-aware in `egret-bin` and passed in, so this module
///   stays IdP-agnostic.
pub fn mount_catalog_engine(
    engine_state: ApiContext<EngineState>,
    store: egret_catalog::service::CatalogStore,
    catalog_jwt: Option<std::sync::Arc<crate::jwt_auth::CatalogJwtAuth>>,
    catalog_public_url: Option<String>,
) -> axum::Router {
    let front = crate::front_auth::FrontAuthState::new(store, catalog_jwt, catalog_public_url);
    catalog_engine_router(engine_state).layer(axum::middleware::from_fn_with_state(
        front,
        crate::front_auth::front_auth,
    ))
}
