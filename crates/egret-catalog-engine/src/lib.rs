//! egret-catalog-engine — spike crate proving lakekeeper's core crates can be
//! embedded as the catalog engine with a single shared `iceberg` version.

pub mod mount;
pub mod pool;
pub mod provision;

pub use mount::{build_state, catalog_engine_router, EngineServer, EngineState};
pub use pool::{build_isolated_pool, LAKEKEEPER_SCHEMA};
pub use provision::{ensure_spike_warehouse, SPIKE_WAREHOUSE_NAME};

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
