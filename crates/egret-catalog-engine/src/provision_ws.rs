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
    CreateWarehouseRequest, ListWarehousesRequest, Service as WarehouseService,
};
use lakekeeper::api::management::v1::ApiServer;
use lakekeeper::api::ApiContext;
use lakekeeper::service::storage::s3::S3AccessKeyCredential;
use lakekeeper::service::storage::{
    S3Credential, S3Flavor, S3Profile, StorageCredential, StorageProfile,
};
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
    // Layer-1 idempotency: if egret already has a mapping, trust it.
    if let Some(existing) = catalog_store
        .warehouse_for_workspace(workspace_id)
        .await
        .map_err(|e| anyhow::anyhow!("reading workspace→warehouse mapping: {e}"))?
    {
        return Ok(existing);
    }

    // The default (nil-UUID) project that `bootstrap` seeded.
    let project_id = ProjectId::new(Uuid::nil());

    // Stable system/admin principal (public lakekeeper/limes APIs only).
    let request_metadata = system_request_metadata(system_user)
        .context("building system principal for warehouse provisioning")?;

    let storage_profile = build_storage_profile(storage_cfg, slug)?;
    let storage_credential = build_storage_credential(storage_cfg);

    let create_req = CreateWarehouseRequest::builder()
        .warehouse_name(slug.to_string())
        .project_id(project_id.clone())
        .storage_profile(storage_profile)
        .storage_credential(storage_credential)
        .build();

    let warehouse_id = match <EngineApiServer as WarehouseService<_, _, _>>::create_warehouse(
        create_req,
        engine_state.clone(),
        request_metadata.clone(),
    )
    .await
    {
        Ok(resp) => *resp.warehouse_id(),
        // Layer-2 idempotency: a conflict means lakekeeper already has a
        // warehouse by this name (CE-1: the overlap check can fire before the
        // name-unique check, so the error type varies — don't parse it, just
        // look the warehouse up by name).
        Err(e) if is_warehouse_conflict(&e) => {
            warehouse_id_by_name(engine_state, request_metadata, project_id, slug)
                .await
                .with_context(|| {
                    format!("recovering existing warehouse `{slug}` after create conflict")
                })?
        }
        Err(e) => {
            return Err(anyhow::anyhow!("create_warehouse `{slug}` failed: {e:?}"));
        }
    };

    // Persist the mapping. `upsert_warehouse` is `ON CONFLICT DO NOTHING`
    // (first-write-wins), so a concurrent provisioner that won the race leaves
    // its id in place — re-read to return the authoritative mapping.
    catalog_store
        .upsert_warehouse(workspace_id, warehouse_id)
        .await
        .map_err(|e| anyhow::anyhow!("persisting workspace→warehouse mapping: {e}"))?;

    let stored = catalog_store
        .warehouse_for_workspace(workspace_id)
        .await
        .map_err(|e| anyhow::anyhow!("re-reading workspace→warehouse mapping: {e}"))?
        .unwrap_or(warehouse_id);
    Ok(stored)
}

/// Build the S3 [`StorageProfile`] for a workspace warehouse from egret's
/// runtime [`StorageConfig`]. Bucket comes from the config; the key prefix
/// namespaces the workspace as `workspaces/<slug>/warehouse`.
fn build_storage_profile(cfg: &StorageConfig, slug: &str) -> anyhow::Result<StorageProfile> {
    let key_prefix = format!("workspaces/{slug}/warehouse");

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

/// Look up an existing warehouse's UUID by name within `project_id`. Used to
/// recover the id after `create_warehouse` reports an already-exists conflict.
async fn warehouse_id_by_name(
    engine_state: ApiContext<EngineState>,
    request_metadata: lakekeeper::api::RequestMetadata,
    project_id: ProjectId,
    name: &str,
) -> anyhow::Result<Uuid> {
    let list_req = ListWarehousesRequest {
        warehouse_status: None,
        project_id: Some(project_id),
    };
    let resp = <EngineApiServer as WarehouseService<_, _, _>>::list_warehouses(
        list_req,
        engine_state,
        request_metadata,
    )
    .await
    .map_err(|e| anyhow::anyhow!("list_warehouses failed: {e:?}"))?;

    resp.warehouses
        .into_iter()
        .find(|w| w.name == name)
        .map(|w| *w.warehouse_id)
        .ok_or_else(|| {
            anyhow::anyhow!("warehouse `{name}` not found after create reported a conflict")
        })
}

/// Whether an error from `create_warehouse` indicates the warehouse already
/// exists (treated as "recover by name lookup").
///
/// CE-1 finding: lakekeeper validates the storage profile for **overlap**
/// against existing warehouses BEFORE the name-unique check, so a duplicate
/// `create_warehouse` with the same name AND the same bucket+prefix can return
/// `CreateWarehouseStorageProfileOverlap` (HTTP 400) rather than a 409 name
/// conflict. We treat any of those as "already provisioned".
fn is_warehouse_conflict(e: &lakekeeper::api::IcebergErrorResponse) -> bool {
    e.error.code == http::StatusCode::CONFLICT.as_u16()
        || e.error.r#type.contains("WarehouseName")
        || e.error.r#type.contains("AlreadyExists")
        || e.error.r#type.contains("StorageProfileOverlap")
}
