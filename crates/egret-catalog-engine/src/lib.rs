//! egret-catalog-engine — spike crate proving lakekeeper's core crates can be
//! embedded as the catalog engine with a single shared `iceberg` version.

/// Minimal scope-based [`Authorizer`](lakekeeper::service::authz::Authorizer)
/// for the embedded engine (CE-4): control-plane actions stay permissive; data
/// actions (namespace/table/view) are gated by the per-token `egret` scope
/// claim. Replaces lakekeeper's inert `AllowAll` authorizer for data actions.
pub mod authorizer;
/// Prod (non-`test-utils`) bootstrap of lakekeeper's default project using a
/// stable admin principal derived from `catalog.system_user`. Replaces the
/// CE-1 spike's `test_user`-based path for production startup.
pub mod bootstrap;
/// Session-authenticated internal catalog browse API (for the UI) reading the
/// embedded lakekeeper catalog (CE-2 T6). Replaces `egret_catalog::browse`,
/// which read egret's now-stale `catalog_*` tables.
pub mod browse;
/// Front-of-router auth + per-workspace token-ownership middleware (CE-2 T5 /
/// A-3) for the embedded lakekeeper Iceberg REST engine.
pub mod front_auth;
pub mod mount;
pub mod pool;
/// Prod (non-`test-utils`) provisioning of a per-workspace lakekeeper warehouse
/// + the egret-side workspace→warehouse mapping. CE-2 T4.
pub mod provision_ws;
// The headless spike provisioner mints a bootstrap principal via
// `RequestMetadata::test_user`, which only exists under lakekeeper's
// `test-utils` feature. Gate the whole module (and its re-export) on the crate's
// `test-utils` feature so the prod build never pulls in that API — only the
// crate's own tests (which enable the feature via the self dev-dependency) see
// it. egret proper will replace `test_user` with a real bootstrap principal.
#[cfg(feature = "test-utils")]
pub mod provision;

pub use authorizer::EgretAuthorizer;
pub use bootstrap::ensure_bootstrapped;
pub use browse::{browse_router, BrowseState};
pub use front_auth::{front_auth, FrontAuthState};
pub use lakekeeper::CancellationToken;
/// Test-only [`build_state`] variant that takes the task-queue poll interval, so
/// integration tests can register the built-in GC queues with a short cadence.
/// Gated behind `test-utils`; `build_state`'s public signature is unchanged.
#[cfg(feature = "test-utils")]
pub use mount::build_state_with_gc_poll;
pub use mount::{
    build_state, catalog_engine_router, mount_catalog_engine, spawn_gc_workers, EngineServer,
    EngineState,
};
pub use pool::{build_isolated_pool, LAKEKEEPER_SCHEMA};
#[cfg(feature = "test-utils")]
pub use provision::{ensure_spike_warehouse, SPIKE_WAREHOUSE_NAME};
pub use provision_ws::provision_workspace_warehouse;

/// Run lakekeeper's Postgres catalog migrations against an isolated
/// `lakekeeper` schema in the database addressed by `dsn`.
///
/// Builds a pool whose connections pin `search_path = lakekeeper, public`
/// (see [`build_isolated_pool`]) and invokes lakekeeper's core migrator on it.
/// Because lakekeeper's DDL and its `_sqlx_migrations` tracker use unqualified
/// names, they all land in `lakekeeper.*`, leaving egret's own migrations in
/// `public.*` untouched.
///
/// This ports the startup call from `lakekeeper-bin`
/// (`lakekeeper_storage_postgres::migrations::migrate_core_only(&write_pool)`),
/// dropping the authorizer/post-migration-hook steps the bin layers on top —
/// the spike only needs the catalog schema in place.
///
/// # Errors
/// Returns an error if the pool cannot be built or any migration fails.
pub async fn run_lakekeeper_migrations(dsn: &str) -> anyhow::Result<()> {
    let pool = build_isolated_pool(dsn).await?;
    // `migrate_core_only` returns the server id; the spike doesn't need it.
    lakekeeper_storage_postgres::migrations::migrate_core_only(&pool).await?;
    pool.close().await;
    Ok(())
}
