//! Provision a lakekeeper warehouse in-process (EB-3). egret's control plane
//! records a `warehouses` row; this drives the embedded lakekeeper to actually
//! create the warehouse (with an S3/RustFS storage profile) so data ops work.

use crate::mount::EngineAuthorizer;
use anyhow::Context as _;
use lakekeeper::api::management::v1::warehouse::{
    CreateWarehouseRequest, Service as WarehouseService, TabularDeleteProfile,
};
use lakekeeper::api::ApiContext;
use lakekeeper::service::storage::{StorageCredential, StorageProfile};
use lakekeeper_storage_postgres::{PostgresBackend, SecretsState};

use crate::bootstrap::system_request_metadata;
use crate::error::CatalogEngineError;
use crate::mount::EngineState;

type EngineApiServer =
    lakekeeper::api::management::v1::ApiServer<PostgresBackend, EngineAuthorizer, SecretsState>;

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

/// The lakekeeper storage profile egret gives every S3 warehouse, in its JSON
/// wire shape.
///
/// Remote request-signing stays OFF: no egret client uses it, and the guard
/// refuses every signer route (rename-authz spec §9, §10.6). Before
/// [`crate::EgretAuthorizer`], lakekeeper's signer asked an allow-all
/// authorizer, so signing on would have signed any table's files for any
/// role with a write grant.
pub fn s3_storage_profile(s3: &S3StorageConfig) -> serde_json::Value {
    serde_json::json!({
        "type": "s3",
        "bucket": s3.bucket,
        "region": s3.region,
        "endpoint": s3.endpoint,
        "path-style-access": s3.path_style,
        "flavor": "s3-compat",
        // STS-vended credentials: on a `vended-credentials` load, lakekeeper calls
        // AssumeRole on the S3 endpoint (RustFS needs no role-arn for s3-compat) with
        // an inline policy downscoped to the table's location, and hands the client
        // short-lived, table-scoped creds. A client that sends no delegation header
        // gets them too; only `client-managed` opts out. Their scope follows the
        // caller's grants: read-only for a read grant, read-write-delete for write
        // or manage ([`crate::EgretAuthorizer`]). Remote request-signing stays off
        // (see above).
        "sts-enabled": true,
        "remote-signing-enabled": false,
        "key-prefix": s3.key_prefix,
    })
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
    refuse_overlapping_storage(&engine_state.v1_state.catalog.read_pool(), s3).await?;

    // Build the storage profile + credential from JSON (the documented wire
    // shape) rather than the many-field builders.
    let storage_profile: StorageProfile =
        serde_json::from_value(s3_storage_profile(s3)).context("building S3 storage profile")?;

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
        EngineAuthorizer,
        SecretsState,
    >>::create_warehouse(request, engine_state, metadata)
    .await
    .map_err(|e| CatalogEngineError::from_lakekeeper("lakekeeper create_warehouse", e))?;

    Ok(response.warehouse_id().to_string())
}

/// Refuse a warehouse on `s3` when its storage overlaps a warehouse the
/// catalog already holds (KI-316). lakekeeper has the same check, but it
/// compares the region and the endpoint as written, so two warehouses on one
/// bucket and prefix slip past it when their storage connections spell the
/// store differently, and a namespace writer in one could then register a
/// table on the other's files. Here the store is the endpoint's host and port
/// ([`store_identity`]); the region does not tell two stores apart. Two host
/// names for one store cannot be told apart without asking DNS, which this
/// does not do.
async fn refuse_overlapping_storage(
    pool: &sqlx::PgPool,
    s3: &S3StorageConfig,
) -> Result<(), CatalogEngineError> {
    // The pool's search path is lakekeeper's schema.
    let held: Vec<(String, serde_json::Value)> =
        sqlx::query_as("SELECT warehouse_name, storage_profile FROM warehouse")
            .fetch_all(pool)
            .await
            .map_err(|e| CatalogEngineError::Internal(format!("listing warehouses: {e}")))?;
    let new = StoreLocation::of(&s3.bucket, s3.endpoint.as_deref(), s3.key_prefix.as_deref());
    for (name, profile) in held {
        if profile.get("type").and_then(|t| t.as_str()) != Some("s3") {
            continue;
        }
        let str_at = |k: &str| profile.get(k).and_then(|v| v.as_str());
        let Some(bucket) = str_at("bucket") else {
            continue;
        };
        if new.overlaps(&StoreLocation::of(
            bucket,
            str_at("endpoint"),
            str_at("key-prefix"),
        )) {
            return Err(CatalogEngineError::Conflict(format!(
                "the storage overlaps warehouse '{name}': the same bucket '{}' on the same \
                 store, with key prefixes that contain one another; give each warehouse its \
                 own bucket or key prefix",
                s3.bucket
            )));
        }
    }
    Ok(())
}

/// Where a warehouse's files live, as [`refuse_overlapping_storage`] compares
/// it.
#[derive(Debug)]
struct StoreLocation {
    store: String,
    bucket: String,
    /// Without leading or trailing `/`; empty for the whole bucket.
    prefix: String,
}

impl StoreLocation {
    fn of(bucket: &str, endpoint: Option<&str>, key_prefix: Option<&str>) -> Self {
        Self {
            store: store_identity(endpoint, bucket),
            bucket: bucket.to_ascii_lowercase(),
            prefix: key_prefix
                .unwrap_or("")
                .trim()
                .trim_matches('/')
                .to_string(),
        }
    }

    fn overlaps(&self, other: &Self) -> bool {
        if self.store != other.store || self.bucket != other.bucket {
            return false;
        }
        let (a, b) = (format!("{}/", self.prefix), format!("{}/", other.prefix));
        self.prefix.is_empty() || other.prefix.is_empty() || a.starts_with(&b) || b.starts_with(&a)
    }
}

/// The object store an S3 endpoint names, one string however the endpoint is
/// written: the host in lower case without a trailing dot, the port (the
/// scheme's default when none is written), and the path as lakekeeper keeps
/// it (without `/` at either end, and without a trailing bucket name, which
/// lakekeeper strips). The scheme is left out: one host and port is one store.
/// No endpoint is the region's AWS endpoint. An endpoint that is not an
/// `http(s)` URL with a host (lakekeeper refuses those) is kept as written, in
/// lower case: a path is never read as a host.
fn store_identity(endpoint: Option<&str>, bucket: &str) -> String {
    let Some(raw) = endpoint.map(str::trim).filter(|e| !e.is_empty()) else {
        return "aws".to_string();
    };
    let Ok(url) = url::Url::parse(raw) else {
        return raw.to_ascii_lowercase();
    };
    let (Some(host), Some(port), true) = (
        url.host_str(),
        url.port_or_known_default(),
        matches!(url.scheme(), "http" | "https"),
    ) else {
        return raw.to_ascii_lowercase();
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let path = url.path().trim_matches('/');
    let path = path.strip_suffix(bucket).unwrap_or(path).trim_matches('/');
    format!("{host}:{port}/{path}")
}

/// Create the lakekeeper warehouse `name` on lakekeeper's in-memory storage
/// (`test-utils` only), for tests that commit tables and views without an
/// object store. Returns the lakekeeper warehouse id.
#[cfg(feature = "test-utils")]
pub async fn create_memory_warehouse(
    engine_state: ApiContext<EngineState>,
    system_user: &str,
    name: &str,
) -> Result<String, CatalogEngineError> {
    let metadata = system_request_metadata(system_user)?;
    let storage_profile: StorageProfile = serde_json::from_value(serde_json::json!({
        "type": "memory",
        "base-location": format!("memory://test-{}", uuid::Uuid::new_v4()),
    }))
    .context("building memory storage profile")?;
    let request = CreateWarehouseRequest::builder()
        .warehouse_name(name.to_string())
        .storage_profile(storage_profile)
        .build();
    let response = <EngineApiServer as WarehouseService<
        PostgresBackend,
        EngineAuthorizer,
        SecretsState,
    >>::create_warehouse(request, engine_state, metadata)
    .await
    .map_err(|e| CatalogEngineError::from_lakekeeper("lakekeeper create_warehouse", e))?;
    Ok(response.warehouse_id().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// KI-316: the spellings of one store are one store.
    #[test]
    fn one_store_written_differently_is_one_store() {
        let id = |e: &str| store_identity(Some(e), "lake");
        let base = id("http://rustfs:9000");
        for same in [
            "http://rustfs:9000/",
            "HTTP://RustFS:9000",
            "http://rustfs.:9000",
            "  http://rustfs:9000  ",
            "https://rustfs:9000",
            "http://rustfs:9000/lake",
            "http://rustfs:9000/lake/",
            "http://user@rustfs:9000",
        ] {
            assert_eq!(id(same), base, "{same}");
        }
        assert_eq!(id("http://s3.local"), id("http://s3.local:80/"));
        assert_eq!(id("https://s3.local"), id("https://s3.local:443"));
        assert_eq!(
            store_identity(None, "lake"),
            store_identity(Some(" "), "lake")
        );
    }

    /// KI-316, and the URL-normaliser lesson: a path is never read as a host,
    /// and different stores stay different.
    #[test]
    fn different_stores_and_paths_stay_apart() {
        let id = |e: &str| store_identity(Some(e), "lake");
        let rustfs = id("http://rustfs:9000");
        for other in [
            "http://rustfs:9001",
            "http://minio:9000",
            "http://proxy/rustfs:9000",
            "http://proxy:9000/rustfs",
            "rustfs:9000",
            "//rustfs:9000",
            "/rustfs:9000",
        ] {
            assert_ne!(id(other), rustfs, "{other}");
        }
        assert_eq!(id("http://proxy/rustfs:9000"), "proxy:80/rustfs:9000");
        assert_ne!(store_identity(None, "lake"), rustfs);
    }

    #[test]
    fn key_prefixes_overlap_when_one_contains_the_other() {
        let at = |e: &str, p: Option<&str>| StoreLocation::of("lake", Some(e), p);
        let a = at("http://rustfs:9000", Some("wh"));
        assert!(a.overlaps(&at("http://rustfs:9000/", Some("/wh/"))));
        assert!(a.overlaps(&at("http://RUSTFS:9000", Some("wh/sub"))));
        assert!(
            a.overlaps(&at("http://rustfs:9000", None)),
            "the whole bucket"
        );
        assert!(
            !a.overlaps(&at("http://rustfs:9000", Some("wh2"))),
            "a sibling prefix"
        );
        assert!(
            !a.overlaps(&at("http://minio:9000", Some("wh"))),
            "another store"
        );
        assert!(!a.overlaps(&StoreLocation::of(
            "other",
            Some("http://rustfs:9000"),
            Some("wh")
        )));
    }

    /// rename-authz spec §9: remote signing off is what keeps the S3 signer
    /// from signing any table's files for any role with a write grant.
    #[test]
    fn egret_s3_warehouses_have_remote_signing_off() {
        let profile = s3_storage_profile(&S3StorageConfig {
            bucket: "b".into(),
            region: "us-east-1".into(),
            endpoint: Some("http://rustfs:9000".into()),
            path_style: true,
            access_key: "AK".into(),
            secret_key: "SK".into(),
            key_prefix: None,
        });
        assert_eq!(profile["remote-signing-enabled"], serde_json::json!(false));
        let parsed: StorageProfile = serde_json::from_value(profile).expect("a storage profile");
        let StorageProfile::S3(s3) = parsed else {
            panic!("an S3 profile")
        };
        assert!(!s3.remote_signing_enabled);
    }
}
