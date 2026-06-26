//! Headless provisioning of a lakekeeper project + warehouse, ported from
//! `lakekeeper-bin/src/main.rs`'s startup sequence — but driven entirely
//! in-process (no HTTP, no UI bootstrap form).
//!
//! ## How project + warehouse creation actually works (KEY FINDING for CE-2)
//!
//! Project and warehouse creation are **not** free functions. They are static
//! methods on the management `Service` traits, implemented by
//! `lakekeeper::api::management::v1::ApiServer<C, A, S>`:
//!
//! * `server::Service::bootstrap(ctx, request_metadata, BootstrapRequest)`
//! * `warehouse::Service::create_warehouse(CreateWarehouseRequest, ctx, request_metadata)`
//! * `project::Service::create_project(CreateProjectRequest, ctx, request_metadata)`
//!
//! Every one takes an `ApiContext<State<A,C,S>>` (our [`EngineState`]) **and a
//! [`RequestMetadata`]** — the authenticated actor. There is no "admin DB
//! function" you can call without an actor; the authorizer is always consulted.
//!
//! ### The actor: `RequestMetadata::new_lakekeeper_internal`
//!
//! The trick for headless calls is `RequestMetadata::new_lakekeeper_internal`
//! — a **public, non-test** constructor producing the in-process
//! `LakekeeperInternal` actor. `bypasses_control_plane_authz` returns `true`
//! for it, so with [`AllowAllAuthorizer`] (and even with a real authorizer)
//! every control-plane action (`bootstrap` / `create_project` /
//! `create_warehouse`) is authorized. This is the same mechanism lakekeeper
//! uses internally for system-initiated mutations.
//!
//! ### Bootstrap seeds the default project for free
//!
//! `bootstrap` flips the catalog's `open_for_bootstrap` flag and — when
//! `enable_default_project` is on (the OSS default → project id = nil UUID) —
//! creates the **default project** itself. So after a successful `bootstrap`
//! the default project already exists; we don't need a separate
//! `create_project` call. (`create_project` is documented above for CE-2's
//! benefit, but is not invoked here.)
//!
//! ### Warehouse creation validates storage at create time
//!
//! `create_warehouse` calls `storage_profile.validate_access(...)`, which
//! performs a **real round-trip against the S3 endpoint** (MinIO here). The
//! bucket must exist and the credentials must be able to write/list it. So this
//! function is a genuine integration step, not a pure DB insert — it needs
//! MinIO reachable at create time.
//!
//! ## Idempotency
//!
//! * `bootstrap` on an already-bootstrapped catalog returns
//!   `CatalogAlreadyBootstrapped`; we treat that as success.
//! * `create_warehouse` with a duplicate name returns a conflict; we look the
//!   warehouse up first and short-circuit if `spike` already exists.

use anyhow::Context as _;
use lakekeeper::api::management::v1::server::{BootstrapRequest, Service as ServerService};
use lakekeeper::api::management::v1::warehouse::{
    CreateWarehouseRequest, Service as WarehouseService,
};
use lakekeeper::api::management::v1::ApiServer;
use lakekeeper::api::ApiContext;
use lakekeeper::api::RequestMetadata;
use lakekeeper::service::storage::s3::S3AccessKeyCredential;
use lakekeeper::service::storage::{
    S3Credential, S3Flavor, S3Profile, StorageCredential, StorageProfile,
};
use lakekeeper::ProjectId;
use lakekeeper_storage_postgres::{PostgresBackend, SecretsState};
use uuid::Uuid;

use crate::mount::EngineState;

/// `ApiServer` specialized to the spike's concrete backends — the type whose
/// `Service` trait impls carry `bootstrap` / `create_warehouse`.
type EngineApiServer =
    ApiServer<PostgresBackend, lakekeeper::service::authz::AllowAllAuthorizer, SecretsState>;

/// Name of the single warehouse the spike provisions.
pub const SPIKE_WAREHOUSE_NAME: &str = "spike";

/// The MinIO bucket the spike warehouse points at. The egret dev MinIO ships a
/// `warehouse` bucket; we namespace the spike under a key prefix inside it.
const SPIKE_BUCKET: &str = "warehouse";
const SPIKE_KEY_PREFIX: &str = "egret-catalog-spike";
const MINIO_ENDPOINT: &str = "http://localhost:9000";
const MINIO_REGION: &str = "local-01";
const MINIO_ACCESS_KEY: &str = "egret";
const MINIO_SECRET_KEY: &str = "egretsecret";

/// Bootstrap the server (seeding the default project) and create the `spike`
/// warehouse with an S3 storage profile pointing at the dev MinIO.
///
/// Returns the warehouse's name (the spike does not yet thread the
/// `WarehouseId` anywhere; Task 4 addresses by name/prefix). Idempotent:
/// re-running against an already-provisioned catalog is a no-op success.
///
/// # Errors
/// Returns an error if bootstrap fails for a reason other than
/// already-bootstrapped, or if warehouse creation fails (including the live
/// MinIO storage-access validation lakekeeper performs at create time).
pub async fn ensure_spike_warehouse(state: ApiContext<EngineState>) -> anyhow::Result<String> {
    // In-process actor that bypasses control-plane authz.
    let request_metadata = RequestMetadata::new_lakekeeper_internal(Uuid::now_v7());

    // 1. Bootstrap. Seeds the default project (nil UUID) when
    //    enable_default_project is on (the OSS default). Tolerate the
    //    already-bootstrapped case for idempotency.
    let bootstrap_req = BootstrapRequest::builder().accept_terms_of_use().build();
    match <EngineApiServer as ServerService<_, _, _>>::bootstrap(
        state.clone(),
        request_metadata.clone(),
        bootstrap_req,
    )
    .await
    {
        Ok(()) => {}
        Err(e) if is_already_bootstrapped(&e) => {
            // Already bootstrapped on a previous run — fine.
        }
        Err(e) => {
            return Err(anyhow::anyhow!("lakekeeper bootstrap failed: {e:?}"));
        }
    }

    // 2. Create the `spike` warehouse in the default project.
    let project_id = ProjectId::new(Uuid::nil());

    let storage_profile = StorageProfile::S3(
        S3Profile::builder()
            .bucket(SPIKE_BUCKET.to_string())
            .key_prefix(SPIKE_KEY_PREFIX.to_string())
            .endpoint(
                MINIO_ENDPOINT
                    .parse()
                    .context("parsing MinIO endpoint URL")?,
            )
            .region(MINIO_REGION.to_string())
            .path_style_access(true)
            .flavor(S3Flavor::S3Compat)
            .sts_enabled(false)
            .build(),
    );

    let storage_credential =
        StorageCredential::S3(S3Credential::AccessKey(S3AccessKeyCredential {
            access_key_id: MINIO_ACCESS_KEY.to_string(),
            secret_access_key: MINIO_SECRET_KEY.to_string(),
            external_id: None,
        }));

    let create_req = CreateWarehouseRequest::builder()
        .warehouse_name(SPIKE_WAREHOUSE_NAME.to_string())
        .project_id(project_id)
        .storage_profile(storage_profile)
        .storage_credential(storage_credential)
        .build();

    match <EngineApiServer as WarehouseService<_, _, _>>::create_warehouse(
        create_req,
        state,
        request_metadata,
    )
    .await
    {
        Ok(_resp) => Ok(SPIKE_WAREHOUSE_NAME.to_string()),
        Err(e) if is_warehouse_conflict(&e) => Ok(SPIKE_WAREHOUSE_NAME.to_string()),
        Err(e) => Err(anyhow::anyhow!("create_warehouse `spike` failed: {e:?}")),
    }
}

/// Whether an error from `bootstrap` indicates the catalog was already
/// bootstrapped (treated as idempotent success).
fn is_already_bootstrapped(e: &lakekeeper::api::IcebergErrorResponse) -> bool {
    let t = &e.error.r#type;
    t == "CatalogAlreadyBootstrapped" || t == "ConcurrentBootstrap"
}

/// Whether an error from `create_warehouse` indicates a name conflict (the
/// warehouse already exists — treated as idempotent success).
fn is_warehouse_conflict(e: &lakekeeper::api::IcebergErrorResponse) -> bool {
    e.error.code == http::StatusCode::CONFLICT.as_u16()
        || e.error.r#type.contains("WarehouseName")
        || e.error.r#type.contains("AlreadyExists")
}
