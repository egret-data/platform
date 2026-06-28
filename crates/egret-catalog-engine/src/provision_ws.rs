//! Prod (non-`test-utils`) provisioning of a per-workspace lakekeeper warehouse.
//!
//! This is the CE-2 T4 counterpart to the CE-1 spike's
//! [`crate::provision::ensure_spike_warehouse`]. The spike hard-coded a single
//! `spike` warehouse with MinIO creds baked in as constants and minted its actor
//! via `RequestMetadata::test_user` (gated behind lakekeeper's `test-utils`).
//! This module instead:
//!
//! * provisions ONE warehouse per workspace (name = workspace slug),
//! * threads the S3 storage profile from egret's runtime [`StorageConfig`],
//! * uses the **stable system principal** from [`crate::bootstrap`] (built from
//!   public lakekeeper/limes APIs only — no `test-utils`),
//! * persists the workspace → warehouse-UUID mapping via the egret-side
//!   [`CatalogStore`], and
//! * is idempotent both at the egret mapping layer and the lakekeeper layer.
//!
//! ## Idempotency (two layers)
//!
//! 1. **egret mapping short-circuit**: if `warehouse_for_workspace(workspace_id)`
//!    already returns `Some(wh)`, we return it without touching lakekeeper. This
//!    is the fast, normal path on re-provision.
//! 2. **lakekeeper create conflict recovery**: if the mapping is absent but
//!    lakekeeper already has a warehouse by that name (e.g. the egret row was
//!    lost, or a previous run committed the warehouse but failed before writing
//!    the mapping), `create_warehouse` returns a name/overlap conflict. Per
//!    CE-1's finding (the overlap check fires before the name-unique check, so
//!    the error type varies), we recover by **looking the warehouse up by name**
//!    via `list_warehouses` rather than relying on parsing the conflict error.
//!
//! ## Storage round-trip at create time
//!
//! `create_warehouse` calls `storage_profile.validate_access(...)`, a real S3
//! round-trip against the configured endpoint (MinIO in dev). The bucket must
//! exist and the credentials must be able to write/list it. So provisioning is a
//! genuine integration step that needs object storage reachable, not a pure DB
//! insert.

use anyhow::Context as _;
use egret_catalog::service::CatalogStore;
use egret_storage::StorageConfig;
use lakekeeper::api::management::v1::warehouse::{
    CreateWarehouseRequest, Service as WarehouseService,
};
use lakekeeper::api::management::v1::{ApiServer, DeleteWarehouseQuery};
use lakekeeper::api::{ApiContext, RequestMetadata};
use lakekeeper::service::storage::s3::S3AccessKeyCredential;
use lakekeeper::service::storage::{
    S3Credential, S3Flavor, S3Profile, StorageCredential, StorageProfile,
};
use lakekeeper::service::WarehouseId;
use lakekeeper::ProjectId;
use lakekeeper_storage_postgres::{PostgresBackend, SecretsState};
use uuid::Uuid;

use crate::authorizer::EgretAuthorizer;
use crate::bootstrap::system_request_metadata;
use crate::mount::EngineState;

/// `ApiServer` specialized to egret's concrete backends — the type whose
/// `Service` trait impls carry `create_warehouse` / `list_warehouses`.
type EngineApiServer = ApiServer<PostgresBackend, EgretAuthorizer, SecretsState>;

/// Provision (idempotently) the lakekeeper warehouse backing `workspace_id` and
/// record the workspace → warehouse-UUID mapping in egret's [`CatalogStore`].
///
/// Returns the warehouse UUID. Safe to call repeatedly for the same workspace:
/// the first call creates the warehouse and persists the mapping; subsequent
/// calls return the same id without re-creating anything.
///
/// * `engine_state` — the lakekeeper [`ApiContext`] built by
///   [`crate::build_state`]. Its catalog must already be migrated and
///   bootstrapped (see [`crate::ensure_bootstrapped`]).
/// * `catalog_store` — egret-side store holding the `workspace_warehouse` map.
/// * `workspace_id` / `slug` — the egret workspace this warehouse backs. The
///   warehouse name mirrors the slug; the S3 key prefix is
///   `workspaces/<slug>/warehouse`.
/// * `storage_cfg` — egret's runtime S3 config (endpoint/region/bucket/creds).
/// * `system_user` — the configured `catalog.system_user`, used to derive the
///   stable bootstrap/admin principal that authorizes the control-plane call.
///
/// # Errors
/// Returns an error if the system principal cannot be built, if
/// `create_warehouse` fails for a reason other than an already-exists conflict
/// (including the live storage `validate_access`), if the post-conflict
/// name lookup cannot find the warehouse, or if persisting the mapping fails.
pub async fn provision_workspace_warehouse(
    engine_state: ApiContext<EngineState>,
    catalog_store: &CatalogStore,
    workspace_id: Uuid,
    slug: &str,
    storage_cfg: &StorageConfig,
    system_user: &str,
) -> anyhow::Result<Uuid> {
    // Idempotency: if the workspace already owns a `'default'` warehouse, return
    // its lakekeeper id without re-creating anything. (Tests reuse this helper to
    // seed catalog data; prod no longer auto-provisions.)
    if let Ok(existing) = catalog_store
        .get_warehouse_by_name(workspace_id, "default")
        .await
    {
        return Ok(existing.warehouse_id);
    }

    // The default (nil-UUID) project that `bootstrap` seeded.
    let project_id = ProjectId::new(Uuid::nil());

    // Stable system/admin principal (public lakekeeper/limes APIs only).
    let request_metadata = system_request_metadata(system_user)
        .context("building system principal for warehouse provisioning")?;

    let warehouse_id = create_warehouse_on(
        engine_state,
        request_metadata,
        project_id,
        slug,
        storage_cfg,
    )
    .await
    .with_context(|| format!("provisioning warehouse for workspace `{slug}`"))?;

    // Persist the egret-side row as the workspace's `'default'` warehouse.
    catalog_store
        .insert_warehouse(workspace_id, warehouse_id, "default", None)
        .await
        .map_err(|e| anyhow::anyhow!("persisting workspace→warehouse mapping: {e}"))?;

    Ok(warehouse_id)
}

/// Create a lakekeeper warehouse named `name` on the storage described by `cfg`,
/// returning its UUID. Reusable building block for the per-warehouse CRUD routes
/// (a workspace now owns N user-managed warehouses, each on a registered storage
/// connection) and for [`provision_workspace_warehouse`].
///
/// The warehouse's S3 key prefix is `warehouses/{name}` (NOT the legacy
/// per-workspace `workspaces/{slug}/warehouse`), so two warehouses on the SAME
/// bucket never collide on their storage location — lakekeeper rejects a
/// create whose profile overlaps an existing warehouse's location.
///
/// `create_warehouse` runs a live `validate_access` S3 round-trip, so a failure
/// here (bad credentials, unreachable endpoint, name/overlap conflict) is a
/// genuine integration error the caller surfaces (the CRUD route maps it to 400).
pub(crate) async fn create_warehouse_on(
    engine_state: ApiContext<EngineState>,
    request_metadata: RequestMetadata,
    project_id: ProjectId,
    name: &str,
    cfg: &StorageConfig,
) -> anyhow::Result<Uuid> {
    let storage_profile = build_warehouse_storage_profile(cfg, name)?;
    let storage_credential = build_storage_credential(cfg);

    let create_req = CreateWarehouseRequest::builder()
        .warehouse_name(name.to_string())
        .project_id(project_id)
        .storage_profile(storage_profile)
        .storage_credential(storage_credential)
        .build();

    let resp = <EngineApiServer as WarehouseService<_, _, _>>::create_warehouse(
        create_req,
        engine_state,
        request_metadata,
    )
    .await
    .map_err(|e| anyhow::anyhow!("create_warehouse `{name}` failed: {e:?}"))?;
    Ok(*resp.warehouse_id())
}

/// Delete a lakekeeper warehouse by UUID (and all its associated resources).
/// Reusable from the per-warehouse CRUD `DELETE` route, run AFTER the egret-side
/// ownership check resolves the lakekeeper id.
pub(crate) async fn delete_warehouse(
    engine_state: ApiContext<EngineState>,
    request_metadata: RequestMetadata,
    warehouse_id: Uuid,
) -> anyhow::Result<()> {
    <EngineApiServer as WarehouseService<_, _, _>>::delete_warehouse(
        WarehouseId::from(warehouse_id),
        // `force` deletes even a protected warehouse; egret warehouses are never
        // protection-flagged, so this only matters defensively.
        DeleteWarehouseQuery { force: true },
        engine_state,
        request_metadata,
    )
    .await
    .map_err(|e| anyhow::anyhow!("delete_warehouse `{warehouse_id}` failed: {e:?}"))
}

/// Build the S3 [`StorageProfile`] for a warehouse named `name`, laid out at the
/// unique `warehouses/{name}` key prefix (so co-bucket warehouses never overlap).
fn build_warehouse_storage_profile(
    cfg: &StorageConfig,
    name: &str,
) -> anyhow::Result<StorageProfile> {
    build_storage_profile_at(cfg, &format!("warehouses/{name}"))
}

/// Build the S3 [`StorageProfile`] for `cfg` at an explicit `key_prefix`. Used by
/// [`build_warehouse_storage_profile`] (per-warehouse `warehouses/<name>` layout).
fn build_storage_profile_at(
    cfg: &StorageConfig,
    key_prefix: &str,
) -> anyhow::Result<StorageProfile> {
    let key_prefix = key_prefix.to_string();

    // A custom endpoint marks an S3-compatible store (MinIO); when absent we are
    // talking to real AWS and leave the endpoint unset. `S3Profile::builder()` is
    // a typed builder whose `.endpoint(..)` changes the builder type, so the two
    // cases are written as separate, complete chains rather than a conditional
    // re-assignment.
    let profile = match cfg.endpoint.as_deref().filter(|e| !e.is_empty()) {
        Some(endpoint) => S3Profile::builder()
            .bucket(cfg.bucket.clone())
            .key_prefix(key_prefix)
            .endpoint(
                endpoint
                    .parse()
                    .with_context(|| format!("parsing storage endpoint URL `{endpoint}`"))?,
            )
            .region(cfg.region.clone())
            .path_style_access(cfg.path_style)
            .flavor(S3Flavor::S3Compat)
            .sts_enabled(false)
            .build(),
        None => S3Profile::builder()
            .bucket(cfg.bucket.clone())
            .key_prefix(key_prefix)
            .region(cfg.region.clone())
            .path_style_access(cfg.path_style)
            .flavor(S3Flavor::Aws)
            .sts_enabled(false)
            .build(),
    };

    Ok(StorageProfile::S3(profile))
}

/// Build the S3 access-key [`StorageCredential`] from egret's [`StorageConfig`].
fn build_storage_credential(cfg: &StorageConfig) -> StorageCredential {
    StorageCredential::S3(S3Credential::AccessKey(S3AccessKeyCredential {
        access_key_id: cfg.access_key.clone(),
        secret_access_key: cfg.secret_key.clone(),
        external_id: None,
    }))
}
