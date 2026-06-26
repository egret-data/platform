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
//!   [`AllowAllAuthorizer`]), `C: CatalogStore` (we use `PostgresBackend`), and
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
//! * `authz` → `AllowAllAuthorizer { server_id }`
//! * `catalog` → `CatalogState::from_pools(read, write)` over our isolated
//!   `lakekeeper`-schema pools.
//! * `secrets` → postgres `SecretsState::from_pools(read, write)`.
//! * `contract_verifiers` → `ContractVerifiers::default()` (no verifiers).
//! * `events` → `EventDispatcher::new(vec![])` (no listeners).
//! * `registered_task_queues` → `TaskQueueRegistry::new().registered_task_queues()`,
//!   an EMPTY registry. The router only needs this handle to answer
//!   `api_config()` when building task-queue management routes; with no queues
//!   registered those routes are simply empty. No worker tasks are spawned
//!   (that is the bin's job in `serve_inner`, which we deliberately skip).
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
use lakekeeper::service::authz::AllowAllAuthorizer;
use lakekeeper::service::contract_verification::ContractVerifiers;
use lakekeeper::service::events::EventDispatcher;
use lakekeeper::service::tasks::TaskQueueRegistry;
use lakekeeper::service::{CatalogStore, State};
use lakekeeper_storage_postgres::{CatalogState, PostgresBackend, SecretsState};

use crate::pool::build_isolated_pool;

/// The concrete state type lakekeeper's router is generic over.
///
/// `A = AllowAllAuthorizer`, `C = PostgresBackend`, `S = SecretsState`.
/// `ApiContext<EngineState>` is what `new_v1_full_router` returns a
/// `Router<_>` for, and the value Task 4 will `.with_state(...)`.
pub type EngineState = State<AllowAllAuthorizer, PostgresBackend, SecretsState>;

/// The concrete service implementation that satisfies the iceberg endpoint
/// traits the router requires (`config`/`namespace`/`tables`/`metrics`/
/// `s3_signer`/`views` `Service`s). Upstream blanket-implements all of them.
pub type EngineServer = CatalogServer<PostgresBackend, AllowAllAuthorizer, SecretsState>;

/// Build the lakekeeper [`ApiContext`] over isolated `lakekeeper`-schema pools.
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
/// # Errors
/// Returns an error if the pools cannot be built or the server info (server id)
/// cannot be read back from the catalog.
pub async fn build_state(dsn: &str) -> anyhow::Result<ApiContext<EngineState>> {
    // Two isolated pools (read + write). Both pin search_path = lakekeeper,public.
    let read_pool = build_isolated_pool(dsn)
        .await
        .context("building isolated read pool")?;
    let write_pool = build_isolated_pool(dsn)
        .await
        .context("building isolated write pool")?;

    let catalog: CatalogState = CatalogState::from_pools(read_pool.clone(), write_pool.clone());
    let secrets: SecretsState = SecretsState::from_pools(read_pool, write_pool);

    // server_id is required to construct the AllowAllAuthorizer. Read it back
    // from the (already-migrated) catalog, exactly like serve.rs does.
    let server_id = <PostgresBackend as CatalogStore>::get_server_info(catalog.clone())
        .await
        .context("reading lakekeeper server info (catalog must be migrated first)")?
        .server_id();
    let authz = AllowAllAuthorizer { server_id };

    // Empty task-queue registry: the router only reads `api_config()` off this
    // handle; we spawn no workers (that is serve_inner's job, which we skip).
    let registered_task_queues = TaskQueueRegistry::new().registered_task_queues();

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

    Ok(ApiContext { v1_state: state })
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
