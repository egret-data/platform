//! egret-catalog-engine — embeds lakekeeper's core crates as the in-process
//! catalog engine with a single shared `iceberg` version.
//!
//! EB-1 reduces this crate to a **bare embedded-lakekeeper mount** fronted by
//! lakekeeper's own permissive [`AllowAllAuthorizer`]. The legacy egret auth /
//! browse / token / warehouse-mapping glue (which depended on egret-catalog's
//! now-deleted warehouse + token store) has been stripped; the real egret
//! [`Authorizer`](lakekeeper::service::authz::Authorizer) and warehouse mapping
//! are re-introduced in EB-3.

/// Prod bootstrap of lakekeeper's default project using a stable admin principal
/// derived from `catalog.system_user`.
pub mod bootstrap;
pub mod browse;
pub mod mount;
pub mod pool;
pub mod recovery;
pub mod warehouse;

pub use bootstrap::ensure_bootstrapped;
pub use browse::{
    list_namespaces, list_tabulars, load_table_detail, load_view_detail, ColumnInfo,
    NamespaceEntry, PartitionField, SnapshotInfo, SortField, TableDetail, TabularEntry,
    TabularKind,
};
pub use lakekeeper::api::ApiContext;
/// lakekeeper's built-in open authorizer. EB-1 mounts the engine with this
/// permissive authorizer; EB-3 replaces it with the real egret authorizer.
pub use lakekeeper::service::authz::AllowAllAuthorizer;
pub use lakekeeper::CancellationToken;
/// Test-only [`build_state`] variant that takes the task-queue poll interval, so
/// integration tests can register the built-in GC queues with a short cadence.
/// Gated behind `test-utils`; `build_state`'s public signature is unchanged.
#[cfg(feature = "test-utils")]
pub use mount::build_state_with_gc_poll;
pub use mount::{
    build_state, catalog_engine_router, mount_catalog_engine, spawn_gc_workers, CatalogGuard,
    EngineServer, EngineState,
};
pub use pool::{build_isolated_pool, LAKEKEEPER_SCHEMA};
pub use recovery::{
    list_deleted_tabulars, set_protection, undrop_tabular, DeletedTabular, ProtectTarget,
    TabularKindName,
};
pub use warehouse::{create_lakekeeper_warehouse, S3StorageConfig};

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
