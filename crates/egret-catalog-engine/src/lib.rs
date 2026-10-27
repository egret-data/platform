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
pub mod admin;
pub mod bootstrap;
pub mod browse;
/// Crate-level engine domain error ([`CatalogEngineError`]) for the per-request
/// warehouse/browse operations — distinct from the store's [`CatalogError`].
pub mod error;
/// The view-governance lineage sweep (spec B3): recompute every view's
/// lineage and re-tag it, on a schedule and on demand.
pub mod lineage_sweep;
pub mod mount;
pub mod pool;
pub mod recovery;
/// The embedded-catalog RBAC store (warehouses/roles/grants/Catalog Clients),
/// merged in from the former `egret-catalog` crate (E3d-1). Runs on the
/// egret-side pool, which is now the same sqlx major lakekeeper uses.
pub mod store;
// D2: recompile StarRocks profiles when a table classified as a whole changes
// schema.
pub mod table_hook;
/// The view-governance DEFINER audit's reader (spec B5): every view in
/// egret's catalogs and its stored `trino.run-as-owner`.
pub mod view_audit;
/// The view-governance hold (spec B2): a classified view's commit waits until
/// every Ranger plugin holds its classification tags.
pub mod view_hold;
pub mod warehouse;
/// The control-plane catalog admin router (warehouses / roles / grants /
/// clients / browse), moved out of `egret-controlplane` into this seam (E3d-3).
/// Nested at `/api/control-plane` in `egret-server`.
pub mod warehouse_admin;

pub use store::{
    CatalogClient, CatalogError, CatalogGrant, CatalogRole, CatalogStore, CreatedClient,
    GrantInput, NewClient, Warehouse,
};

pub use admin::{
    get_warehouse_config, list_tasks, rotate_storage_credential, search_tabulars,
    set_delete_profile, set_format_versions, set_warehouse_active, SearchResult, TaskInfo,
    WarehouseConfig,
};
pub use bootstrap::ensure_bootstrapped;
pub use browse::{
    list_namespaces, list_tabulars, load_table_detail, load_view_detail, ColumnInfo,
    NamespaceEntry, PartitionField, SnapshotInfo, SortField, TableDetail, TabularEntry,
    TabularKind,
};
pub use error::CatalogEngineError;
pub use lakekeeper::api::ApiContext;
/// lakekeeper's built-in open authorizer. EB-1 mounts the engine with this
/// permissive authorizer; EB-3 replaces it with the real egret authorizer.
pub use lakekeeper::service::authz::AllowAllAuthorizer;
pub use lakekeeper::CancellationToken;
pub use lineage_sweep::{
    spawn_classification_sweeps, LineageSweep, LineageSweeper, NoCatalogSweeper, SweepError,
    SweepFailure, SweepReport, LINEAGE_SWEEP_INTERVAL,
};
/// Test-only [`build_state`] variant that takes the task-queue poll interval, so
/// integration tests can register the built-in GC queues with a short cadence.
/// Gated behind `test-utils`; `build_state`'s public signature is unchanged.
#[cfg(feature = "test-utils")]
pub use mount::build_state_with_gc_poll;
pub use mount::{
    build_state, catalog_engine_router, front_catalog_router, mount_catalog_engine,
    spawn_gc_workers, CatalogCaller, CatalogGuard, EngineServer, EngineState, GuardedBody,
};
pub use pool::{build_isolated_pool, LAKEKEEPER_SCHEMA};
pub use recovery::{
    list_deleted_tabulars, set_protection, undrop_tabular, DeletedTabular, ProtectTarget,
    TabularKindName,
};
pub use table_hook::TableHook;
pub use view_audit::{
    load_view_run_as_owner, CatalogView, CatalogViews, EmbeddedViewCatalog, NoViewCatalog,
    ViewCatalog, RUN_AS_OWNER_PROPERTY,
};
pub use view_hold::{
    default_hold_limit, view_lock_key, ViewHold, DEFAULT_MAX_HOLDS, EXPLAIN_BUDGET,
    LINEAGE_REFUSAL, UNCONFIRMED_REFUSAL,
};
#[cfg(feature = "test-utils")]
pub use warehouse::create_memory_warehouse;
pub use warehouse::{create_lakekeeper_warehouse, s3_storage_profile, S3StorageConfig};
pub use warehouse_admin::{
    catalog_admin_router, BrowseColumn, BrowseDeletedTabular, BrowseNamespace, BrowsePartition,
    BrowseSearchResult, BrowseSnapshot, BrowseSort, BrowseTableDetail, BrowseTabular, BrowseTask,
    BrowseWarehouseConfig, CatalogAdminApi, CatalogAdminState, CatalogHttpError, ProvisionS3,
    WarehouseBrowser, WarehouseProvisioner,
};

/// The Catalog seam's OpenAPI document (E0·P4b-4): the warehouse-management
/// admin router ([`catalog_admin_router`], mounted at `/api/control-plane`).
/// `egret-server` calls this and merges it into the top-level API doc — the
/// per-crate `openapi()` convention every annotated seam exposes.
///
/// Only the warehouse-admin (FE-facing) router is expressible here. The
/// data-plane [`catalog_engine_router`] is lakekeeper's opaque Iceberg REST
/// router (`new_v1_full_router`) with no egret handler functions to carry
/// `#[utoipa::path]`, and is engine-facing (Trino/Spark/Daft via catalog
/// tokens), not browser-facing — so it is intentionally left out of the doc.
pub fn openapi() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi as _;
    CatalogAdminApi::openapi()
}

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
