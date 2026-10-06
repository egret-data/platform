//! Provision a lakekeeper warehouse in-process (EB-3). egret's control plane
//! records a `warehouses` row; this drives the embedded lakekeeper to actually
//! create the warehouse (with an S3/RustFS storage profile) so data ops work.

use anyhow::Context as _;
use lakekeeper::api::management::v1::warehouse::{
    CreateWarehouseRequest, Service as WarehouseService, TabularDeleteProfile,
};
use lakekeeper::api::ApiContext;
use lakekeeper::service::authz::AllowAllAuthorizer;
use lakekeeper::service::storage::{StorageCredential, StorageProfile};
use lakekeeper_storage_postgres::{PostgresBackend, SecretsState};

use crate::bootstrap::system_request_metadata;
use crate::error::CatalogEngineError;
use crate::mount::EngineState;

type EngineApiServer =
    lakekeeper::api::management::v1::ApiServer<PostgresBackend, AllowAllAuthorizer, SecretsState>;

/// S3-compatible storage config for a warehouse (RustFS in dev).
#[derive(Debug, Clone)]
pub struct S3StorageConfig {
    pub bucket: String,
    pub region: String,
    /// S3 endpoint (e.g. RustFS `http://rustfs:9000`); `None` uses the region default.
    pub endpoint: Option<String>,
    pub path_style: bool,
    pub access_key: String,
    pub secret_key: String,
    pub key_prefix: Option<String>,
}

/// Create the lakekeeper warehouse `name` backed by `s3`, using the in-process
/// system admin principal. Returns the lakekeeper warehouse id (as a string) to
/// record on egret's `warehouses` row.
pub async fn create_lakekeeper_warehouse(
    engine_state: ApiContext<EngineState>,
    system_user: &str,
    name: &str,
    s3: &S3StorageConfig,
) -> Result<String, CatalogEngineError> {
    let metadata = system_request_metadata(system_user)?;

    // Build the storage profile + credential from JSON (the documented wire
    // shape) rather than the many-field builders.
    let storage_profile: StorageProfile = serde_json::from_value(serde_json::json!({
        "type": "s3",
        "bucket": s3.bucket,
        "region": s3.region,
        "endpoint": s3.endpoint,
        "path-style-access": s3.path_style,
        "flavor": "s3-compat",
        // STS-vended credentials: on a `vended-credentials` load, lakekeeper calls
        // AssumeRole on the S3 endpoint (RustFS needs no role-arn for s3-compat) with
        // an inline policy downscoped to the table's location, and hands the client
        // short-lived, table-scoped creds. Clients that don't request delegation fall
        // back to their own static access key. Remote request-signing stays off — the
        // embedded mount exposes no `/sign` endpoint.
        "sts-enabled": true,
        "remote-signing-enabled": false,
        "key-prefix": s3.key_prefix,
    }))
    .context("building S3 storage profile")?;

    let storage_credential: StorageCredential = serde_json::from_value(serde_json::json!({
        "type": "s3",
        "credential-type": "access-key",
        "access-key-id": s3.access_key,
        "secret-access-key": s3.secret_key,
    }))
    .context("building S3 storage credential")?;

    let request = CreateWarehouseRequest::builder()
        .warehouse_name(name.to_string())
        .storage_profile(storage_profile)
        .storage_credential(storage_credential)
        // Soft-delete so dropped tables are recoverable (the "Recycle bin") for a
        // week before the GC purge queue reclaims their storage.
        .delete_profile(TabularDeleteProfile::Soft {
            expiration_seconds: chrono::Duration::days(7),
        })
        .build();

    let response = <EngineApiServer as WarehouseService<
        PostgresBackend,
        AllowAllAuthorizer,
        SecretsState,
    >>::create_warehouse(request, engine_state, metadata)
    .await
    .map_err(|e| CatalogEngineError::from_lakekeeper("lakekeeper create_warehouse", e))?;

    Ok(response.warehouse_id().to_string())
}
