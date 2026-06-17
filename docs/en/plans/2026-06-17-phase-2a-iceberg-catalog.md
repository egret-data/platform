# Phase 2a — Iceberg REST Catalog Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a spec-compliant Iceberg REST Catalog (`egret-catalog`) backed by Postgres + an object-store/signing crate (`egret-storage`), so engines (pyiceberg/Trino/StarRocks/Spark) use Egret as their catalog with read-write tables and S3 remote signing.

**Architecture:** Three new crates — `egret-storage` (S3 via aws-sdk-s3 + SigV4 remote signing via aws-sigv4), `egret-iceberg-ext` (Iceberg REST request/response DTOs, mirroring lakekeeper's `iceberg-ext`), and `egret-catalog` (axum HTTP layer + Postgres `CatalogStore` + commit engine). Postgres is the source of truth: the full `TableMetadata` is stored as one `jsonb` column; `metadata.json` is also materialized to object storage. Commits use optimistic concurrency (`FOR UPDATE` row lock + a `current_version` CAS). Mounted at `/catalog`, authenticated by a static bearer token. Multi-workspace tenancy is deferred.

**Tech Stack:** Rust (workspace, edition 2021, MSRV 1.85) · axum 0.8 · sqlx 0.8 (Postgres) · `iceberg` (lakekeeper fork, rev `bb7017339f778de44830b4f970d9d5e1e84f5599`) · aws-sdk-s3 1.121 · aws-sigv4 1.2 · MinIO (S3) · pyiceberg (real-client test) · testcontainers.

**Reference implementation:** `data-portal/lakekeeper` — mirror `crates/iceberg-ext` (DTOs), `crates/lakekeeper/src/server/commit_tables.rs` (commit), `crates/lakekeeper/src/server/s3_signer/sign.rs` + `crates/io/src/s3.rs` (signing/client). Spec: [`docs/en/specs/2026-06-17-phase-2a-iceberg-catalog-design.md`](../specs/2026-06-17-phase-2a-iceberg-catalog-design.md).

**Conventions to follow (from the existing codebase):**
- Crates live in `crates/<name>/`, registered in the root `Cargo.toml` `[workspace.dependencies]` with `path = "crates/<name>"`; each crate's `Cargo.toml` uses `edition.workspace = true`, `license.workspace = true`, and `<dep>.workspace = true`.
- Migrations: plain SQL files `migrations/000N_name.sql`, `gen_random_uuid()`, `timestamptz NOT NULL DEFAULT now()`. The embedded `MIGRATOR` test in `egret-metastore` asserts `>= 5`; bump it.
- DB-gated integration tests live in `crates/<name>/tests/*.rs` and self-skip when `DATABASE_URL` is unset (see `crates/egret-auth/tests/session_db.rs`). Storage tests additionally self-skip when the `[storage]` endpoint is unreachable.
- Config: add nested structs to `egret_config::Config` with a `Default` impl and `validate()` checks; tests use `figment::Jail`.
- Server wiring: `egret_server::AppState` carries `Option<_>` sub-states; `build_router` mounts a router when the state is `Some`.
- **CRITICAL — sqlx style:** this codebase uses **runtime sqlx**, NOT the compile-time-checked `!` macros. The plan's SQL snippets show `sqlx::query!` / `sqlx::query_scalar!` for brevity, but you MUST implement them with the runtime forms used everywhere in `egret-auth`/`egret-workspace`: `sqlx::query_as::<_, Row>(...)` with a `#[derive(sqlx::FromRow)]` row struct, `sqlx::query_scalar::<_, T>(...)`, and `sqlx::query(...)` — using `.bind(arg)` for parameters (no `$1` interpolation of Rust values; bind positionally). This avoids needing a live `DATABASE_URL` or `.sqlx` cache at build time. Map columns explicitly. Example from `crates/egret-auth/src/users.rs`: `sqlx::query_as::<_, UserRow>("SELECT ... WHERE id = $1").bind(id).fetch_optional(&pool).await?`. For `text[]` and `jsonb` params, bind `Vec<String>` and `serde_json::Value` directly (sqlx supports both with the enabled features).

---

## Task 0: Workspace dependencies & crate skeletons

**Files:**
- Modify: `Cargo.toml` (root `[workspace.dependencies]`)
- Create: `crates/egret-storage/Cargo.toml`, `crates/egret-storage/src/lib.rs`
- Create: `crates/egret-iceberg-ext/Cargo.toml`, `crates/egret-iceberg-ext/src/lib.rs`
- Create: `crates/egret-catalog/Cargo.toml`, `crates/egret-catalog/src/lib.rs`

- [ ] **Step 1: Add new workspace deps + crate path entries to root `Cargo.toml`**

Under `[workspace.dependencies]` add the three crate paths and the external deps:

```toml
egret-storage = { path = "crates/egret-storage" }
egret-iceberg-ext = { path = "crates/egret-iceberg-ext" }
egret-catalog = { path = "crates/egret-catalog" }

# Iceberg model — pin the lakekeeper fork/rev so the TableMetadataBuilder /
# TableUpdate::apply / TableRequirement::check API matches the reference exactly.
iceberg = { git = "https://github.com/lakekeeper/iceberg-rust.git", rev = "bb7017339f778de44830b4f970d9d5e1e84f5599" }
aws-config = { version = "1", features = ["behavior-version-latest"] }
aws-credential-types = { version = "1.2" }
aws-sdk-s3 = { version = "1.121", default-features = false, features = ["sigv4a", "http-1x", "default-https-client", "rt-tokio"] }
aws-sdk-sts = { version = "1", default-features = false, features = ["default-https-client", "rt-tokio"] }
aws-sigv4 = { version = "1.2" }
url = { version = "2", features = ["serde"] }
http = "1"
typed-builder = "0.20"
```

- [ ] **Step 2: Create the three crate `Cargo.toml` files**

`crates/egret-storage/Cargo.toml`:
```toml
[package]
name = "egret-storage"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
egret-core.workspace = true
aws-config.workspace = true
aws-credential-types.workspace = true
aws-sdk-s3.workspace = true
aws-sigv4.workspace = true
async-trait.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tracing.workspace = true
url.workspace = true
http.workspace = true

[dev-dependencies]
tokio.workspace = true
```

`crates/egret-iceberg-ext/Cargo.toml`:
```toml
[package]
name = "egret-iceberg-ext"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
iceberg.workspace = true
serde.workspace = true
serde_json.workspace = true
uuid.workspace = true
url.workspace = true
http.workspace = true

[dev-dependencies]
serde_json.workspace = true
```

`crates/egret-catalog/Cargo.toml`:
```toml
[package]
name = "egret-catalog"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
egret-core.workspace = true
egret-config.workspace = true
egret-metastore.workspace = true
egret-storage.workspace = true
egret-iceberg-ext.workspace = true
iceberg.workspace = true
axum.workspace = true
sqlx.workspace = true
serde.workspace = true
serde_json.workspace = true
chrono.workspace = true
uuid.workspace = true
thiserror.workspace = true
anyhow.workspace = true
tracing.workspace = true
url.workspace = true
http.workspace = true
async-trait.workspace = true

[dev-dependencies]
tokio.workspace = true
tower.workspace = true
http-body-util = "0.1"
egret-config.workspace = true
```

- [ ] **Step 3: Create placeholder `lib.rs` for each crate so the workspace builds**

`crates/egret-storage/src/lib.rs`:
```rust
//! Object-store abstraction (S3/MinIO) and S3 SigV4 remote signing for the catalog.
```
`crates/egret-iceberg-ext/src/lib.rs`:
```rust
//! Iceberg REST Catalog request/response DTOs, mirroring lakekeeper's `iceberg-ext`.
```
`crates/egret-catalog/src/lib.rs`:
```rust
//! Iceberg REST Catalog: HTTP layer + Postgres CatalogStore + commit engine.
```

- [ ] **Step 4: Verify the workspace builds**

Run: `cargo build -p egret-storage -p egret-iceberg-ext -p egret-catalog`
Expected: compiles (downloads the `iceberg` git dep). If `iceberg` fails to resolve, confirm network access to GitHub and the pinned rev.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/egret-storage crates/egret-iceberg-ext crates/egret-catalog
git commit -m "feat(catalog): scaffold egret-storage, egret-iceberg-ext, egret-catalog crates"
```

---

## Task 1: `egret-storage` — ObjectStore trait + S3Store (MinIO)

**Files:**
- Create: `crates/egret-storage/src/config.rs`, `crates/egret-storage/src/error.rs`, `crates/egret-storage/src/s3.rs`, `crates/egret-storage/src/location.rs`
- Modify: `crates/egret-storage/src/lib.rs`
- Test: `crates/egret-storage/tests/s3_minio.rs`

- [ ] **Step 1: Define config + error + the `ObjectStore` trait**

`crates/egret-storage/src/config.rs`:
```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    /// Custom S3 endpoint (MinIO). Empty/None means real AWS.
    pub endpoint: Option<String>,
    pub region: String,
    pub bucket: String,
    pub access_key: String,
    pub secret_key: String,
    /// Required for MinIO.
    pub path_style: bool,
}
```

`crates/egret-storage/src/error.rs`:
```rust
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("object not found: {0}")]
    NotFound(String),
    #[error("s3 error: {0}")]
    S3(String),
    #[error("signing error: {0}")]
    Signing(String),
}
```

`crates/egret-storage/src/lib.rs`:
```rust
//! Object-store abstraction (S3/MinIO) and S3 SigV4 remote signing for the catalog.
mod config;
mod error;
mod location;
mod s3;
pub mod signer;

pub use config::StorageConfig;
pub use error::StorageError;
pub use location::{table_base_location, table_metadata_location};
pub use s3::S3Store;

use async_trait::async_trait;

/// Minimal object-store surface the catalog needs (write/read/delete metadata files).
#[async_trait]
pub trait ObjectStore: Send + Sync {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), StorageError>;
    async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError>;
    async fn delete(&self, uri: &str) -> Result<(), StorageError>;
}
```

- [ ] **Step 2: Implement warehouse-location helpers (pure, unit-testable)**

`crates/egret-storage/src/location.rs`:
```rust
//! Warehouse layout: <warehouse>/<ns-levels...>/<table>-<uuid>/(metadata|data)/...

/// Base location for a new table, e.g. `s3://wh/sales/orders-<uuid>`.
pub fn table_base_location(warehouse: &str, namespace: &[String], table: &str, table_uuid: &uuid::Uuid) -> String {
    let wh = warehouse.trim_end_matches('/');
    let ns = namespace.join("/");
    if ns.is_empty() {
        format!("{wh}/{table}-{table_uuid}")
    } else {
        format!("{wh}/{ns}/{table}-{table_uuid}")
    }
}

/// Metadata file location for version N, e.g. `<base>/metadata/00003-<uuid>.metadata.json`.
pub fn table_metadata_location(base_location: &str, version: i32, file_uuid: &uuid::Uuid) -> String {
    let base = base_location.trim_end_matches('/');
    format!("{base}/metadata/{version:05}-{file_uuid}.metadata.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn base_location_joins_namespace_levels() {
        let id = uuid::Uuid::nil();
        assert_eq!(table_base_location("s3://wh/", &["sales".into()], "orders", &id),
                   "s3://wh/sales/orders-00000000-0000-0000-0000-000000000000");
    }
    #[test]
    fn metadata_location_zero_pads_version() {
        let id = uuid::Uuid::nil();
        let m = table_metadata_location("s3://wh/sales/orders-x", 3, &id);
        assert!(m.ends_with("/metadata/00003-00000000-0000-0000-0000-000000000000.metadata.json"), "{m}");
    }
}
```
(Add `uuid` to `egret-storage` deps if the compiler flags it: `uuid.workspace = true`.)

- [ ] **Step 3: Run the location unit tests**

Run: `cargo test -p egret-storage location`
Expected: PASS (2 tests).

- [ ] **Step 4: Implement `S3Store` (aws-sdk-s3, MinIO endpoint + path-style)**

`crates/egret-storage/src/s3.rs` — parse `s3://bucket/key` URIs and call get/put/delete. Mirror lakekeeper `crates/io/src/s3.rs` client config (endpoint_url + force_path_style):
```rust
use crate::{ObjectStore, StorageConfig, StorageError};
use async_trait::async_trait;

#[derive(Clone)]
pub struct S3Store {
    client: aws_sdk_s3::Client,
}

impl S3Store {
    pub async fn new(cfg: &StorageConfig) -> Self {
        let creds = aws_credential_types::Credentials::new(
            cfg.access_key.clone(), cfg.secret_key.clone(), None, None, "egret-storage");
        let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(cfg.region.clone()))
            .credentials_provider(creds);
        if let Some(ep) = &cfg.endpoint {
            loader = loader.endpoint_url(ep.clone());
        }
        let shared = loader.load().await;
        let mut b = aws_sdk_s3::config::Builder::from(&shared);
        if cfg.path_style {
            b = b.force_path_style(true);
        }
        Self { client: aws_sdk_s3::Client::from_conf(b.build()) }
    }

    /// Split `s3://bucket/key` → (bucket, key).
    fn split(uri: &str) -> Result<(String, String), StorageError> {
        let rest = uri.strip_prefix("s3://").ok_or_else(|| StorageError::S3(format!("not an s3 uri: {uri}")))?;
        let (bucket, key) = rest.split_once('/').ok_or_else(|| StorageError::S3(format!("missing key: {uri}")))?;
        Ok((bucket.to_string(), key.to_string()))
    }
}

#[async_trait]
impl ObjectStore for S3Store {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), StorageError> {
        let (bucket, key) = Self::split(uri)?;
        self.client.put_object().bucket(bucket).key(key).body(bytes.into())
            .send().await.map_err(|e| StorageError::S3(e.to_string()))?;
        Ok(())
    }
    async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError> {
        let (bucket, key) = Self::split(uri)?;
        let out = self.client.get_object().bucket(bucket).key(key).send().await
            .map_err(|e| StorageError::S3(e.to_string()))?;
        let data = out.body.collect().await.map_err(|e| StorageError::S3(e.to_string()))?;
        Ok(data.to_vec())
    }
    async fn delete(&self, uri: &str) -> Result<(), StorageError> {
        let (bucket, key) = Self::split(uri)?;
        self.client.delete_object().bucket(bucket).key(key).send().await
            .map_err(|e| StorageError::S3(e.to_string()))?;
        Ok(())
    }
}
```

- [ ] **Step 5: Write the MinIO round-trip integration test (gated)**

`crates/egret-storage/tests/s3_minio.rs`:
```rust
//! Gated S3 round-trip. Skips unless EGRET_TEST_S3_ENDPOINT is set (e.g. MinIO via `just up`).
use egret_storage::{ObjectStore, S3Store, StorageConfig};

fn cfg() -> Option<StorageConfig> {
    let endpoint = std::env::var("EGRET_TEST_S3_ENDPOINT").ok()?;
    Some(StorageConfig {
        endpoint: Some(endpoint),
        region: "us-east-1".into(),
        bucket: std::env::var("EGRET_TEST_S3_BUCKET").unwrap_or_else(|_| "warehouse".into()),
        access_key: std::env::var("EGRET_TEST_S3_KEY").unwrap_or_else(|_| "minioadmin".into()),
        secret_key: std::env::var("EGRET_TEST_S3_SECRET").unwrap_or_else(|_| "minioadmin".into()),
        path_style: true,
    })
}

#[tokio::test]
async fn put_get_delete_round_trip() {
    let Some(c) = cfg() else { eprintln!("EGRET_TEST_S3_ENDPOINT unset — skipping"); return; };
    let bucket = c.bucket.clone();
    let store = S3Store::new(&c).await;
    let uri = format!("s3://{bucket}/egret-test/{}.txt", uuid::Uuid::new_v4());
    store.put(&uri, b"hello".to_vec()).await.unwrap();
    assert_eq!(store.get(&uri).await.unwrap(), b"hello");
    store.delete(&uri).await.unwrap();
    assert!(store.get(&uri).await.is_err());
}
```
(Add `uuid` to `egret-storage` `[dev-dependencies]` if needed.)

- [ ] **Step 6: Run the storage test (skips without MinIO, passes)**

Run: `cargo test -p egret-storage`
Expected: location tests PASS; `put_get_delete_round_trip` prints "skipping" and PASSES when `EGRET_TEST_S3_ENDPOINT` is unset. With MinIO up (`EGRET_TEST_S3_ENDPOINT=http://localhost:9000`), it round-trips.

- [ ] **Step 7: Commit**

```bash
git add crates/egret-storage
git commit -m "feat(storage): ObjectStore trait + S3Store (MinIO) + location helpers"
```

---

## Task 2: `egret-storage::signer` — S3 SigV4 remote signing

**Files:**
- Create: `crates/egret-storage/src/signer.rs`
- Test: unit tests inline in `signer.rs`

- [ ] **Step 1: Define the sign request/response types + `sign_s3_request`**

Mirror lakekeeper `crates/lakekeeper/src/server/s3_signer/sign.rs`. The DTO field names match the Iceberg REST `S3SignRequest`/`S3SignResponse`.

`crates/egret-storage/src/signer.rs`:
```rust
//! S3 remote signing: take an unsigned request shape from the client, return SigV4 headers.
use crate::{StorageConfig, StorageError};
use aws_sigv4::http_request::{sign as aws_sign, SignableBody, SignableRequest, SigningSettings, PercentEncodingMode, PayloadChecksumKind};
use aws_sigv4::sign::v4;
use std::collections::HashMap;
use std::time::SystemTime;

/// Headers the client manages itself — never include in the signature.
const UNSIGNED_HEADERS: &[&str] = &["range", "x-amz-date", "amz-sdk-invocation-id", "amz-sdk-retry"];

#[derive(serde::Deserialize)]
pub struct S3SignRequest {
    pub region: String,
    pub uri: url::Url,
    #[serde(with = "http_serde::method")]
    pub method: http::Method,
    pub headers: HashMap<String, Vec<String>>,
    pub body: Option<String>,
}

#[derive(serde::Serialize)]
pub struct S3SignResponse {
    pub uri: url::Url,
    pub headers: HashMap<String, Vec<String>>,
}

/// Sign `req` with the catalog's S3 credentials; return the URI + headers to send to S3.
pub async fn sign_s3_request(cfg: &StorageConfig, req: &S3SignRequest) -> Result<S3SignResponse, StorageError> {
    let body_bytes = req.body.clone().map(String::into_bytes);
    let signable_body = match &body_bytes {
        Some(b) => SignableBody::Bytes(b),
        None => SignableBody::UnsignedPayload,
    };

    let mut settings = SigningSettings::default();
    settings.percent_encoding_mode = PercentEncodingMode::Single;
    settings.payload_checksum_kind = PayloadChecksumKind::XAmzSha256;

    let creds = aws_credential_types::Credentials::new(
        cfg.access_key.clone(), cfg.secret_key.clone(), None, None, "egret-signer");
    let identity = creds.into();

    let params = v4::SigningParams::builder()
        .identity(&identity)
        .region(&req.region)
        .name("s3")
        .time(SystemTime::now())
        .settings(settings)
        .build()
        .map_err(|e| StorageError::Signing(e.to_string()))?
        .into();

    let mut header_pairs: Vec<(String, String)> = Vec::new();
    for (k, values) in &req.headers {
        if UNSIGNED_HEADERS.contains(&k.to_ascii_lowercase().as_str()) { continue; }
        for v in values { header_pairs.push((k.clone(), v.clone())); }
    }

    let signable = SignableRequest::new(
        req.method.as_str(),
        req.uri.to_string(),
        header_pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())),
        signable_body,
    ).map_err(|e| StorageError::Signing(e.to_string()))?;

    let (instructions, _sig) = aws_sign(signable, &params)
        .map_err(|e| StorageError::Signing(e.to_string()))?
        .into_parts();

    let mut out_uri = req.uri.clone();
    for (k, v) in instructions.params() { out_uri.query_pairs_mut().append_pair(k, v); }
    let mut out_headers = req.headers.clone();
    for (k, v) in instructions.headers() { out_headers.insert(k.to_string(), vec![v.to_string()]); }

    Ok(S3SignResponse { uri: out_uri, headers: out_headers })
}
```
Add deps to `egret-storage/Cargo.toml`: `http-serde = "1"`, `serde.workspace = true`, `url.workspace = true` (already present). If `http_serde::method` is unavailable for this `http` version, implement a small `mod http_method_serde` that (de)serializes `http::Method` via its string form (see lakekeeper `s3_signer.rs` `http_method_serde`).

- [ ] **Step 2: Unit-test the signer produces an AWS4 Authorization header**

Append to `crates/egret-storage/src/signer.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    fn cfg() -> StorageConfig {
        StorageConfig { endpoint: Some("http://localhost:9000".into()), region: "us-east-1".into(),
            bucket: "warehouse".into(), access_key: "AKIDEXAMPLE".into(),
            secret_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(), path_style: true }
    }
    #[tokio::test]
    async fn signs_put_with_authorization_header() {
        let req = S3SignRequest {
            region: "us-east-1".into(),
            uri: url::Url::parse("http://localhost:9000/warehouse/ns/t/data/f.parquet").unwrap(),
            method: http::Method::PUT,
            headers: std::collections::HashMap::from([("x-amz-content-sha256".to_string(), vec!["UNSIGNED-PAYLOAD".to_string()])]),
            body: None,
        };
        let resp = sign_s3_request(&cfg(), &req).await.unwrap();
        let auth = resp.headers.get("authorization").or_else(|| resp.headers.get("Authorization"));
        assert!(auth.map(|v| v[0].starts_with("AWS4-HMAC-SHA256")).unwrap_or(false),
                "expected AWS4 auth header, got {:?}", resp.headers);
    }
}
```

- [ ] **Step 3: Run the signer unit test**

Run: `cargo test -p egret-storage signer`
Expected: PASS. If the `authorization` header key casing differs, adjust the assertion to be case-insensitive (the test already checks both).

- [ ] **Step 4: Commit**

```bash
git add crates/egret-storage
git commit -m "feat(storage): S3 SigV4 remote signing (sign_s3_request)"
```

---

## Task 3: `egret-iceberg-ext` — REST DTOs

**Files:**
- Create: `crates/egret-iceberg-ext/src/catalog.rs`, `crates/egret-iceberg-ext/src/sign.rs`, `crates/egret-iceberg-ext/src/error.rs`
- Modify: `crates/egret-iceberg-ext/src/lib.rs`
- Test: inline serde round-trip tests

These mirror lakekeeper `crates/iceberg-ext/src/catalog/rest/*`. Use `iceberg` types for the spec payloads so `TableMetadata` (de)serializes correctly.

- [ ] **Step 1: Define the DTOs**

`crates/egret-iceberg-ext/src/lib.rs`:
```rust
//! Iceberg REST Catalog request/response DTOs, mirroring lakekeeper's `iceberg-ext`.
pub mod catalog;
pub mod error;
pub mod sign;

pub use iceberg::{NamespaceIdent, TableIdent, TableRequirement, TableUpdate};
pub use iceberg::spec::{Schema, SortOrder, TableMetadata, TableMetadataRef, UnboundPartitionSpec};
```

`crates/egret-iceberg-ext/src/catalog.rs` (field shapes copied from the lakekeeper structs — keep the serde renames exact):
```rust
use std::collections::HashMap;
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use iceberg::{NamespaceIdent, TableIdent, TableRequirement, TableUpdate};
use iceberg::spec::{Schema, SortOrder, TableMetadataRef, UnboundPartitionSpec};

#[derive(Clone, Default, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CatalogConfig {
    pub overrides: HashMap<String, String>,
    pub defaults: HashMap<String, String>,
    pub endpoints: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateNamespaceRequest {
    pub namespace: NamespaceIdent,
    pub properties: Option<HashMap<String, String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateNamespaceResponse {
    pub namespace: NamespaceIdent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<HashMap<String, String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct GetNamespaceResponse {
    pub namespace: NamespaceIdent,
    pub properties: Option<HashMap<String, String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ListNamespacesResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_page_token: Option<String>,
    pub namespaces: Vec<NamespaceIdent>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct UpdateNamespacePropertiesRequest {
    pub removals: Option<Vec<String>>,
    pub updates: Option<HashMap<String, String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct UpdateNamespacePropertiesResponse {
    pub updated: Vec<String>,
    pub removed: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missing: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CreateTableRequest {
    pub name: String,
    pub location: Option<String>,
    pub schema: Schema,
    pub partition_spec: Option<UnboundPartitionSpec>,
    pub write_order: Option<SortOrder>,
    pub stage_create: Option<bool>,
    pub properties: Option<HashMap<String, String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LoadTableResult {
    pub metadata_location: Option<String>,
    pub metadata: TableMetadataRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<HashMap<String, String>>,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CommitTableRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identifier: Option<TableIdent>,
    pub requirements: Vec<TableRequirement>,
    pub updates: Vec<TableUpdate>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CommitTableResponse {
    pub metadata_location: String,
    pub metadata: TableMetadataRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<HashMap<String, String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ListTablesResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_page_token: Option<String>,
    pub identifiers: Vec<TableIdent>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct RegisterTableRequest {
    pub name: String,
    pub metadata_location: String,
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct RenameTableRequest {
    pub source: TableIdent,
    pub destination: TableIdent,
}

// silence unused-import for Arc if not used after trimming
#[allow(unused_imports)]
use Arc as _Arc;
```
(If `Arc` is unused, drop the import; it is shown only because the upstream types use it.)

`crates/egret-iceberg-ext/src/error.rs`:
```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct IcebergErrorResponse {
    pub error: ErrorModel,
}

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct ErrorModel {
    pub message: String,
    pub r#type: String,
    pub code: u16,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub stack: Vec<String>,
}

impl ErrorModel {
    pub fn new(message: impl Into<String>, r#type: impl Into<String>, code: u16) -> Self {
        Self { message: message.into(), r#type: r#type.into(), code, stack: vec![] }
    }
}
```

`crates/egret-iceberg-ext/src/sign.rs`:
```rust
use std::collections::HashMap;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct S3SignRequest {
    pub region: String,
    pub uri: url::Url,
    #[serde(with = "http_serde::method")]
    pub method: http::Method,
    pub headers: HashMap<String, Vec<String>>,
    pub body: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct S3SignResponse {
    pub uri: url::Url,
    pub headers: HashMap<String, Vec<String>>,
}
```
Add `http-serde = "1"` to `egret-iceberg-ext` deps. (These mirror the `egret-storage::signer` types; the catalog handler will convert between them, or you may re-export the storage ones — keep them separate to avoid a storage→ext dependency cycle.)

- [ ] **Step 2: Add a serde round-trip test for a real TableMetadata + CommitTableRequest**

`crates/egret-iceberg-ext/src/catalog.rs` (append):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_response_uses_kebab_case() {
        let c = CatalogConfig { overrides: std::collections::HashMap::from([("warehouse".into(), "s3://wh".into())]),
            defaults: Default::default(), endpoints: vec![] };
        let j = serde_json::to_value(&c).unwrap();
        assert_eq!(j["overrides"]["warehouse"], "s3://wh");
    }
    #[test]
    fn commit_request_deserializes_requirements_and_updates() {
        // Minimal commit: assert-create requirement + set-properties update.
        let body = serde_json::json!({
            "requirements": [{ "type": "assert-create" }],
            "updates": [{ "action": "set-properties", "updates": { "owner": "egret" } }]
        });
        let req: CommitTableRequest = serde_json::from_value(body).unwrap();
        assert_eq!(req.requirements.len(), 1);
        assert_eq!(req.updates.len(), 1);
    }
}
```
If the exact tag for `assert-create` / `set-properties` differs in the pinned `iceberg` rev, copy a known-good JSON from lakekeeper's `crates/iceberg-ext/src/catalog/rest/table.rs` tests and use that.

- [ ] **Step 3: Run the DTO tests**

Run: `cargo test -p egret-iceberg-ext`
Expected: PASS (2 tests).

- [ ] **Step 4: Commit**

```bash
git add crates/egret-iceberg-ext
git commit -m "feat(iceberg-ext): Iceberg REST DTOs (config, namespace, table, sign, error)"
```

---

## Task 4: Migration `0006_catalog.sql` + bump MIGRATOR assertion

**Files:**
- Create: `migrations/0006_catalog.sql`
- Modify: `crates/egret-metastore/src/lib.rs` (the `>= 5` assertion → `>= 6`)

- [ ] **Step 1: Write the migration**

`migrations/0006_catalog.sql` (exact SQL from spec §8):
```sql
CREATE TABLE catalog_namespaces (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    levels     text[] NOT NULL,
    properties jsonb  NOT NULL DEFAULT '{}',
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (levels)
);

CREATE TABLE catalog_tables (
    id                         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    namespace_id               uuid NOT NULL REFERENCES catalog_namespaces(id) ON DELETE RESTRICT,
    name                       text NOT NULL,
    table_uuid                 uuid NOT NULL,
    metadata                   jsonb NOT NULL,
    metadata_location          text NOT NULL,
    previous_metadata_location text,
    location                   text NOT NULL,
    current_snapshot_id        bigint,
    current_version            int  NOT NULL DEFAULT 0,
    created_at                 timestamptz NOT NULL DEFAULT now(),
    updated_at                 timestamptz NOT NULL DEFAULT now(),
    UNIQUE (namespace_id, name)
);

CREATE TABLE catalog_table_metadata_log (
    id                uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    table_id          uuid NOT NULL REFERENCES catalog_tables(id) ON DELETE CASCADE,
    metadata_location text NOT NULL,
    created_at        timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE catalog_views (
    id                uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    namespace_id      uuid NOT NULL REFERENCES catalog_namespaces(id) ON DELETE RESTRICT,
    name              text NOT NULL,
    metadata          jsonb NOT NULL,
    metadata_location text NOT NULL,
    location          text NOT NULL,
    current_version   int  NOT NULL DEFAULT 0,
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now(),
    UNIQUE (namespace_id, name)
);

-- forward-compat; NOT enforced this phase (tenancy deferred)
CREATE TABLE workspace_catalog_grant (
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    namespace_id uuid NOT NULL REFERENCES catalog_namespaces(id) ON DELETE CASCADE,
    granted_by   uuid REFERENCES users(id),
    granted_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, namespace_id)
);
```

- [ ] **Step 2: Bump the embedded-migration assertion**

In `crates/egret-metastore/src/lib.rs`, change:
```rust
        assert!(
            MIGRATOR.migrations.len() >= 6,
            "expected through the catalog migration"
        );
```

- [ ] **Step 3: Verify migration compiles into the binary**

Run: `cargo test -p egret-metastore`
Expected: PASS (the `sqlx::migrate!` macro embeds `0006`; assertion now `>= 6`).

- [ ] **Step 4: Commit**

```bash
git add migrations/0006_catalog.sql crates/egret-metastore/src/lib.rs
git commit -m "feat(catalog): 0006_catalog.sql schema (namespaces/tables/views/grant)"
```

---

## Task 5: `CatalogStore` — namespaces CRUD (Postgres, DB-gated)

**Files:**
- Create: `crates/egret-catalog/src/service/mod.rs`, `crates/egret-catalog/src/service/store.rs`, `crates/egret-catalog/src/error.rs`
- Modify: `crates/egret-catalog/src/lib.rs`
- Test: `crates/egret-catalog/tests/store_ns.rs`

- [ ] **Step 1: Define `CatalogError` + `CatalogStore` with namespace methods**

`crates/egret-catalog/src/error.rs`:
```rust
#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("namespace not empty: {0}")]
    NamespaceNotEmpty(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Storage(#[from] egret_storage::StorageError),
}
```

`crates/egret-catalog/src/lib.rs`:
```rust
//! Iceberg REST Catalog: HTTP layer + Postgres CatalogStore + commit engine.
pub mod error;
pub mod service;

pub use error::CatalogError;
```

`crates/egret-catalog/src/service/mod.rs`:
```rust
pub mod store;
pub use store::CatalogStore;
```

`crates/egret-catalog/src/service/store.rs` (namespace methods):
```rust
use crate::CatalogError;
use egret_metastore::Db;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Clone)]
pub struct CatalogStore {
    db: Db,
}

pub struct NamespaceRow {
    pub id: Uuid,
    pub levels: Vec<String>,
    pub properties: serde_json::Value,
}

impl CatalogStore {
    pub fn new(db: Db) -> Self { Self { db } }

    pub async fn create_namespace(&self, levels: &[String], props: &HashMap<String, String>)
        -> Result<NamespaceRow, CatalogError> {
        let props_json = serde_json::to_value(props).unwrap();
        let rec = sqlx::query!(
            r#"INSERT INTO catalog_namespaces (levels, properties) VALUES ($1, $2)
               RETURNING id, levels, properties"#,
            levels, props_json)
            .fetch_one(&self.db.pool).await
            .map_err(|e| match &e {
                sqlx::Error::Database(d) if d.is_unique_violation() =>
                    CatalogError::AlreadyExists(levels.join(".")),
                _ => CatalogError::Db(e),
            })?;
        Ok(NamespaceRow { id: rec.id, levels: rec.levels, properties: rec.properties })
    }

    pub async fn get_namespace(&self, levels: &[String]) -> Result<NamespaceRow, CatalogError> {
        let rec = sqlx::query!(
            r#"SELECT id, levels, properties FROM catalog_namespaces WHERE levels = $1"#, levels)
            .fetch_optional(&self.db.pool).await?
            .ok_or_else(|| CatalogError::NotFound(levels.join(".")))?;
        Ok(NamespaceRow { id: rec.id, levels: rec.levels, properties: rec.properties })
    }

    pub async fn list_namespaces(&self, parent: Option<&[String]>) -> Result<Vec<Vec<String>>, CatalogError> {
        let rows = match parent {
            None => sqlx::query_scalar!(r#"SELECT levels FROM catalog_namespaces ORDER BY levels"#)
                .fetch_all(&self.db.pool).await?,
            Some(p) => sqlx::query_scalar!(
                r#"SELECT levels FROM catalog_namespaces WHERE levels[1:cardinality($1::text[])] = $1 AND cardinality(levels) = cardinality($1::text[]) + 1 ORDER BY levels"#, p)
                .fetch_all(&self.db.pool).await?,
        };
        Ok(rows)
    }

    pub async fn drop_namespace(&self, levels: &[String]) -> Result<(), CatalogError> {
        let ns = self.get_namespace(levels).await?;
        let count: i64 = sqlx::query_scalar!(
            r#"SELECT count(*) as "c!" FROM catalog_tables WHERE namespace_id = $1"#, ns.id)
            .fetch_one(&self.db.pool).await?;
        if count > 0 { return Err(CatalogError::NamespaceNotEmpty(levels.join("."))); }
        sqlx::query!(r#"DELETE FROM catalog_namespaces WHERE id = $1"#, ns.id)
            .execute(&self.db.pool).await?;
        Ok(())
    }

    pub async fn update_namespace_properties(&self, levels: &[String],
        removals: &[String], updates: &HashMap<String, String>)
        -> Result<(Vec<String>, Vec<String>), CatalogError> {
        let ns = self.get_namespace(levels).await?;
        let mut props: HashMap<String, String> =
            serde_json::from_value(ns.properties).unwrap_or_default();
        let mut removed = vec![];
        for k in removals { if props.remove(k).is_some() { removed.push(k.clone()); } }
        let mut updated = vec![];
        for (k, v) in updates { props.insert(k.clone(), v.clone()); updated.push(k.clone()); }
        let json = serde_json::to_value(&props).unwrap();
        sqlx::query!(r#"UPDATE catalog_namespaces SET properties = $2 WHERE id = $1"#, ns.id, json)
            .execute(&self.db.pool).await?;
        Ok((updated, removed))
    }
}
```
Note: `sqlx::query!` requires compile-time DB access or `SQLX_OFFLINE=true` with a prepared `.sqlx/` cache. The existing crates use `sqlx::query!` — follow whatever the repo already does (check for `.sqlx/` dir or a `DATABASE_URL` at build). If the repo builds offline, run `cargo sqlx prepare` for `egret-catalog` after writing queries.

- [ ] **Step 2: Write DB-gated namespace tests**

`crates/egret-catalog/tests/store_ns.rs`:
```rust
//! DB-gated CatalogStore namespace tests. Self-skip without DATABASE_URL.
use egret_catalog::service::CatalogStore;
use egret_metastore::Db;
use std::collections::HashMap;

async fn store() -> Option<CatalogStore> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(CatalogStore::new(db))
}
fn ns() -> Vec<String> { vec![format!("t_{}", uuid::Uuid::new_v4().simple())] }

#[tokio::test]
async fn create_get_drop_namespace() {
    let Some(s) = store().await else { eprintln!("skip"); return; };
    let levels = ns();
    s.create_namespace(&levels, &HashMap::from([("k".into(), "v".into())])).await.unwrap();
    let got = s.get_namespace(&levels).await.unwrap();
    assert_eq!(got.levels, levels);
    s.drop_namespace(&levels).await.unwrap();
    assert!(s.get_namespace(&levels).await.is_err());
}

#[tokio::test]
async fn duplicate_namespace_is_already_exists() {
    let Some(s) = store().await else { eprintln!("skip"); return; };
    let levels = ns();
    s.create_namespace(&levels, &HashMap::new()).await.unwrap();
    let err = s.create_namespace(&levels, &HashMap::new()).await.unwrap_err();
    assert!(matches!(err, egret_catalog::CatalogError::AlreadyExists(_)));
    s.drop_namespace(&levels).await.unwrap();
}
```

- [ ] **Step 3: Run the namespace tests**

Run: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-catalog store_ns`
Expected: PASS with DB up; "skip" + PASS without `DATABASE_URL`.

- [ ] **Step 4: Commit**

```bash
git add crates/egret-catalog
git commit -m "feat(catalog): CatalogStore namespace CRUD (DB-gated)"
```

---

## Task 6: `CatalogStore` — tables (create/load/list/drop/rename) + CAS

**Files:**
- Modify: `crates/egret-catalog/src/service/store.rs`
- Test: `crates/egret-catalog/tests/store_tables.rs`

- [ ] **Step 1: Add a `TableRow` + table methods (create/load/list/drop/rename, version CAS)**

Append to `store.rs`:
```rust
use iceberg::spec::TableMetadata;

pub struct TableRow {
    pub id: Uuid,
    pub name: String,
    pub table_uuid: Uuid,
    pub metadata: TableMetadata,
    pub metadata_location: String,
    pub location: String,
    pub current_version: i32,
}

impl CatalogStore {
    /// Insert a freshly-created table (version 0). `metadata` already has its uuid/location.
    pub async fn insert_table(&self, ns_id: Uuid, name: &str, table_uuid: Uuid,
        metadata: &TableMetadata, metadata_location: &str, location: &str)
        -> Result<(), CatalogError> {
        let md = serde_json::to_value(metadata).map_err(|e| CatalogError::BadRequest(e.to_string()))?;
        let snap = metadata.current_snapshot_id();
        sqlx::query!(
            r#"INSERT INTO catalog_tables
                 (namespace_id, name, table_uuid, metadata, metadata_location, location, current_snapshot_id, current_version)
               VALUES ($1,$2,$3,$4,$5,$6,$7,0)"#,
            ns_id, name, table_uuid, md, metadata_location, location, snap)
            .execute(&self.db.pool).await
            .map_err(|e| match &e {
                sqlx::Error::Database(d) if d.is_unique_violation() => CatalogError::AlreadyExists(name.into()),
                _ => CatalogError::Db(e),
            })?;
        Ok(())
    }

    pub async fn load_table(&self, ns_id: Uuid, name: &str) -> Result<TableRow, CatalogError> {
        let rec = sqlx::query!(
            r#"SELECT id, name, table_uuid, metadata, metadata_location, location, current_version
               FROM catalog_tables WHERE namespace_id = $1 AND name = $2"#, ns_id, name)
            .fetch_optional(&self.db.pool).await?
            .ok_or_else(|| CatalogError::NotFound(name.into()))?;
        let metadata: TableMetadata = serde_json::from_value(rec.metadata)
            .map_err(|e| CatalogError::BadRequest(e.to_string()))?;
        Ok(TableRow { id: rec.id, name: rec.name, table_uuid: rec.table_uuid, metadata,
            metadata_location: rec.metadata_location, location: rec.location,
            current_version: rec.current_version })
    }

    pub async fn list_tables(&self, ns_id: Uuid) -> Result<Vec<String>, CatalogError> {
        Ok(sqlx::query_scalar!(
            r#"SELECT name FROM catalog_tables WHERE namespace_id = $1 ORDER BY name"#, ns_id)
            .fetch_all(&self.db.pool).await?)
    }

    pub async fn drop_table(&self, ns_id: Uuid, name: &str) -> Result<TableRow, CatalogError> {
        let row = self.load_table(ns_id, name).await?;
        sqlx::query!(r#"DELETE FROM catalog_tables WHERE id = $1"#, row.id)
            .execute(&self.db.pool).await?;
        Ok(row)
    }

    pub async fn rename_table(&self, src_ns: Uuid, src: &str, dst_ns: Uuid, dst: &str)
        -> Result<(), CatalogError> {
        let row = self.load_table(src_ns, src).await?;
        sqlx::query!(
            r#"UPDATE catalog_tables SET namespace_id = $2, name = $3, updated_at = now() WHERE id = $1"#,
            row.id, dst_ns, dst)
            .execute(&self.db.pool).await
            .map_err(|e| match &e {
                sqlx::Error::Database(d) if d.is_unique_violation() => CatalogError::AlreadyExists(dst.into()),
                _ => CatalogError::Db(e),
            })?;
        Ok(())
    }

    /// Compare-and-swap commit: write new metadata only if `current_version` is unchanged.
    /// Returns Err(Conflict) if another writer advanced the version.
    pub async fn cas_commit(&self, table_id: Uuid, expected_version: i32,
        metadata: &TableMetadata, new_metadata_location: &str)
        -> Result<(), CatalogError> {
        let md = serde_json::to_value(metadata).map_err(|e| CatalogError::BadRequest(e.to_string()))?;
        let snap = metadata.current_snapshot_id();
        let mut tx = self.db.pool.begin().await?;
        let affected = sqlx::query!(
            r#"UPDATE catalog_tables
                 SET metadata = $3, previous_metadata_location = metadata_location,
                     metadata_location = $4, current_snapshot_id = $5,
                     current_version = current_version + 1, updated_at = now()
               WHERE id = $1 AND current_version = $2"#,
            table_id, expected_version, md, new_metadata_location, snap)
            .execute(&mut *tx).await?.rows_affected();
        if affected == 0 {
            tx.rollback().await?;
            return Err(CatalogError::Conflict("stale table version".into()));
        }
        sqlx::query!(
            r#"INSERT INTO catalog_table_metadata_log (table_id, metadata_location) VALUES ($1, $2)"#,
            table_id, new_metadata_location)
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
}
```
Confirm the exact `TableMetadata` accessor for the current snapshot id in the pinned rev (`current_snapshot_id()` returns `Option<i64>` in iceberg-rust; if the name differs, check `iceberg::spec::TableMetadata`).

- [ ] **Step 2: DB-gated test for insert/load/list/drop/rename + CAS conflict**

`crates/egret-catalog/tests/store_tables.rs` — build a minimal `TableMetadata` via `TableMetadataBuilder::new(...)` (see Task 7 for the exact builder call), insert it, then:
```rust
//! DB-gated table-store tests. Self-skip without DATABASE_URL.
use egret_catalog::service::CatalogStore;
use egret_catalog::CatalogError;
use egret_metastore::Db;
use std::collections::HashMap;

async fn store() -> Option<CatalogStore> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(CatalogStore::new(db))
}

// Reuse the helper from Task 7's commit module to build minimal metadata.
fn minimal_metadata(location: &str, id: uuid::Uuid) -> iceberg::spec::TableMetadata {
    egret_catalog::service::test_support::minimal_table_metadata(location, id)
}

#[tokio::test]
async fn insert_load_drop_table() {
    let Some(s) = store().await else { eprintln!("skip"); return; };
    let levels = vec![format!("t_{}", uuid::Uuid::new_v4().simple())];
    let ns = s.create_namespace(&levels, &HashMap::new()).await.unwrap();
    let tid = uuid::Uuid::new_v4();
    let loc = format!("s3://wh/{}/orders-{tid}", levels[0]);
    let md = minimal_metadata(&loc, tid);
    let mloc = format!("{loc}/metadata/00000-{}.metadata.json", uuid::Uuid::new_v4());
    s.insert_table(ns.id, "orders", tid, &md, &mloc, &loc).await.unwrap();
    let row = s.load_table(ns.id, "orders").await.unwrap();
    assert_eq!(row.current_version, 0);
    assert_eq!(s.list_tables(ns.id).await.unwrap(), vec!["orders".to_string()]);
    s.drop_table(ns.id, "orders").await.unwrap();
    assert!(s.load_table(ns.id, "orders").await.is_err());
    s.drop_namespace(&levels).await.unwrap();
}

#[tokio::test]
async fn cas_commit_rejects_stale_version() {
    let Some(s) = store().await else { eprintln!("skip"); return; };
    let levels = vec![format!("t_{}", uuid::Uuid::new_v4().simple())];
    let ns = s.create_namespace(&levels, &HashMap::new()).await.unwrap();
    let tid = uuid::Uuid::new_v4();
    let loc = format!("s3://wh/{}/t-{tid}", levels[0]);
    let md = minimal_metadata(&loc, tid);
    let mloc = format!("{loc}/metadata/00000-x.metadata.json");
    s.insert_table(ns.id, "t", tid, &md, &mloc, &loc).await.unwrap();
    let row = s.load_table(ns.id, "t").await.unwrap();
    // First commit at version 0 succeeds.
    s.cas_commit(row.id, 0, &md, &format!("{loc}/metadata/00001-y.metadata.json")).await.unwrap();
    // Second commit still using version 0 must conflict.
    let err = s.cas_commit(row.id, 0, &md, &format!("{loc}/metadata/00002-z.metadata.json")).await.unwrap_err();
    assert!(matches!(err, CatalogError::Conflict(_)));
    s.drop_table(ns.id, "t").await.unwrap();
    s.drop_namespace(&levels).await.unwrap();
}
```

- [ ] **Step 3: Run table-store tests (after Task 7's `test_support` exists, or inline a builder here)**

Run: `DATABASE_URL=... cargo test -p egret-catalog store_tables`
Expected: PASS with DB. If you implement this before Task 7, inline the `minimal_table_metadata` builder here instead of referencing `test_support`.

- [ ] **Step 4: Commit**

```bash
git add crates/egret-catalog
git commit -m "feat(catalog): CatalogStore tables (create/load/list/drop/rename) + CAS commit"
```

---

## Task 7: Commit engine + metadata I/O (requirements check + apply updates)

**Files:**
- Create: `crates/egret-catalog/src/service/commit.rs`, `crates/egret-catalog/src/service/metadata.rs`, `crates/egret-catalog/src/service/test_support.rs`
- Modify: `crates/egret-catalog/src/service/mod.rs`
- Test: inline unit tests in `commit.rs`

- [ ] **Step 1: Implement `create_table_metadata` + `apply_commit` using the iceberg builder**

Mirror lakekeeper `crates/lakekeeper/src/server/commit_tables.rs` and `create_table.rs`.

`crates/egret-catalog/src/service/metadata.rs`:
```rust
use crate::CatalogError;
use egret_iceberg_ext::catalog::CreateTableRequest;
use iceberg::spec::{FormatVersion, SortOrder, TableMetadata, TableMetadataBuilder, UnboundPartitionSpec};
use uuid::Uuid;

/// Build the initial TableMetadata for a CREATE TABLE request (format v2, assigned uuid + location).
pub fn create_table_metadata(req: &CreateTableRequest, table_uuid: Uuid, location: String)
    -> Result<TableMetadata, CatalogError> {
    let partition_spec = req.partition_spec.clone()
        .unwrap_or_else(|| UnboundPartitionSpec::builder().build());
    let sort_order = req.write_order.clone().unwrap_or_else(SortOrder::unsorted_order);
    let props = req.properties.clone().unwrap_or_default();
    let built = TableMetadataBuilder::new(
            req.schema.clone(), partition_spec, sort_order, location,
            FormatVersion::V2, props)
        .map_err(|e| CatalogError::BadRequest(e.to_string()))?
        .assign_uuid(table_uuid)
        .build()
        .map_err(|e| CatalogError::BadRequest(e.to_string()))?;
    Ok(built.metadata)
}
```

`crates/egret-catalog/src/service/commit.rs` (builder construction mirrors lakekeeper `commit_tables.rs`: `TableMetadataBuilder::new_from_metadata(metadata, metadata_location.map(ToString::to_string))`):
```rust
use crate::CatalogError;
use iceberg::spec::{TableMetadata, TableMetadataBuilder};
use iceberg::{TableRequirement, TableUpdate};

/// Validate requirements against current metadata, then apply updates → new metadata.
/// `metadata_location` is None for a staged/just-created table (affects assert-create).
pub fn apply_commit(current: TableMetadata, metadata_location: Option<&str>,
    requirements: &[TableRequirement], updates: Vec<TableUpdate>)
    -> Result<TableMetadata, CatalogError> {
    // 1. Requirements (optimistic concurrency). Pass Some(&metadata) only when the table
    //    already exists (has a metadata_location); None signals a staged/new table.
    for r in requirements {
        r.check(metadata_location.map(|_| &current))
            .map_err(|e| CatalogError::Conflict(e.to_string()))?;
    }
    // 2. Apply updates in order via the iceberg builder.
    let mut builder = TableMetadataBuilder::new_from_metadata(
        current, metadata_location.map(str::to_string));
    for u in updates {
        builder = TableUpdate::apply(u, builder).map_err(|e| CatalogError::BadRequest(e.to_string()))?;
    }
    let built = builder.build().map_err(|e| CatalogError::Conflict(e.to_string()))?;
    Ok(built.metadata)
}
```

`crates/egret-catalog/src/service/test_support.rs`:
```rust
//! Test-only helpers shared by integration tests.
use iceberg::spec::{FormatVersion, NestedField, PrimitiveType, Schema, SortOrder, TableMetadata, TableMetadataBuilder, Type, UnboundPartitionSpec};
use std::sync::Arc;
use uuid::Uuid;

/// Build a minimal single-column (id: long) table metadata for tests.
pub fn minimal_table_metadata(location: &str, table_uuid: Uuid) -> TableMetadata {
    let schema = Schema::builder()
        .with_fields(vec![Arc::new(NestedField::required(1, "id", Type::Primitive(PrimitiveType::Long)))])
        .build().unwrap();
    TableMetadataBuilder::new(schema, UnboundPartitionSpec::builder().build(),
        SortOrder::unsorted_order(), location.to_string(), FormatVersion::V2, Default::default())
        .unwrap().assign_uuid(table_uuid).build().unwrap().metadata
}
```
(Verify `NestedField::required`, `Schema::builder().with_fields`, and `Type::Primitive(PrimitiveType::Long)` against the pinned iceberg rev; these are the standard iceberg-rust constructors.)

`crates/egret-catalog/src/service/mod.rs`:
```rust
pub mod store;
pub mod commit;
pub mod metadata;
pub mod test_support;
pub use store::CatalogStore;
```

- [ ] **Step 2: Unit-test apply_commit (set-properties update with no requirements)**

Append to `commit.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::test_support::minimal_table_metadata;
    use std::collections::HashMap;

    #[test]
    fn apply_set_properties_update() {
        let md = minimal_table_metadata("s3://wh/ns/t-x", uuid::Uuid::nil());
        let updates = vec![TableUpdate::SetProperties {
            updates: HashMap::from([("owner".to_string(), "egret".to_string())]) }];
        let new_md = apply_commit(md, Some("s3://wh/ns/t-x/metadata/00000-x.json"), &[], updates).unwrap();
        assert_eq!(new_md.properties().get("owner").map(String::as_str), Some("egret"));
    }
}
```
Confirm the `TableUpdate::SetProperties { updates }` variant name and `TableMetadata::properties()` accessor against the pinned rev (both match the agent-extracted API).

- [ ] **Step 3: Run the commit unit test**

Run: `cargo test -p egret-catalog commit`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/egret-catalog
git commit -m "feat(catalog): commit engine (apply_commit) + create_table_metadata + test_support"
```

---

## Task 8: `CatalogStore` views + commit_table service method

**Files:**
- Modify: `crates/egret-catalog/src/service/store.rs` (view methods)
- Create: `crates/egret-catalog/src/service/service.rs` (orchestration: create/commit table writing metadata.json via storage)
- Modify: `crates/egret-catalog/src/service/mod.rs`
- Test: `crates/egret-catalog/tests/store_views.rs`

- [ ] **Step 1: Add view CRUD methods to `CatalogStore`** (mirror the table methods, against `catalog_views`)

Append to `store.rs`:
```rust
pub struct ViewRow { pub id: Uuid, pub name: String, pub metadata: serde_json::Value,
    pub metadata_location: String, pub location: String, pub current_version: i32 }

impl CatalogStore {
    pub async fn insert_view(&self, ns_id: Uuid, name: &str, metadata: &serde_json::Value,
        metadata_location: &str, location: &str) -> Result<(), CatalogError> {
        sqlx::query!(r#"INSERT INTO catalog_views (namespace_id, name, metadata, metadata_location, location, current_version)
                        VALUES ($1,$2,$3,$4,$5,0)"#, ns_id, name, metadata, metadata_location, location)
            .execute(&self.db.pool).await
            .map_err(|e| match &e { sqlx::Error::Database(d) if d.is_unique_violation() =>
                CatalogError::AlreadyExists(name.into()), _ => CatalogError::Db(e) })?;
        Ok(())
    }
    pub async fn load_view(&self, ns_id: Uuid, name: &str) -> Result<ViewRow, CatalogError> {
        let rec = sqlx::query!(r#"SELECT id, name, metadata, metadata_location, location, current_version
                                  FROM catalog_views WHERE namespace_id=$1 AND name=$2"#, ns_id, name)
            .fetch_optional(&self.db.pool).await?.ok_or_else(|| CatalogError::NotFound(name.into()))?;
        Ok(ViewRow { id: rec.id, name: rec.name, metadata: rec.metadata,
            metadata_location: rec.metadata_location, location: rec.location, current_version: rec.current_version })
    }
    pub async fn list_views(&self, ns_id: Uuid) -> Result<Vec<String>, CatalogError> {
        Ok(sqlx::query_scalar!(r#"SELECT name FROM catalog_views WHERE namespace_id=$1 ORDER BY name"#, ns_id)
            .fetch_all(&self.db.pool).await?)
    }
    pub async fn drop_view(&self, ns_id: Uuid, name: &str) -> Result<(), CatalogError> {
        let v = self.load_view(ns_id, name).await?;
        sqlx::query!(r#"DELETE FROM catalog_views WHERE id=$1"#, v.id).execute(&self.db.pool).await?;
        Ok(())
    }
}
```

- [ ] **Step 2: Add the orchestration layer** that writes `metadata.json` to storage and calls the store. This is what the HTTP handlers call.

`crates/egret-catalog/src/service/service.rs`:
```rust
use crate::service::{commit::apply_commit, metadata::create_table_metadata, CatalogStore};
use crate::CatalogError;
use egret_iceberg_ext::catalog::{CommitTableRequest, CreateTableRequest, LoadTableResult};
use egret_storage::{table_metadata_location, ObjectStore};
use iceberg::spec::TableMetadata;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone)]
pub struct CatalogService {
    pub store: CatalogStore,
    pub storage: Arc<dyn ObjectStore>,
}

impl CatalogService {
    async fn write_metadata_json(&self, location: &str, version: i32, md: &TableMetadata)
        -> Result<String, CatalogError> {
        let mloc = table_metadata_location(location, version, &Uuid::new_v4());
        let bytes = serde_json::to_vec(md).map_err(|e| CatalogError::BadRequest(e.to_string()))?;
        self.storage.put(&mloc, bytes).await?;
        Ok(mloc)
    }

    pub async fn create_table(&self, ns_levels: &[String], req: CreateTableRequest)
        -> Result<LoadTableResult, CatalogError> {
        let ns = self.store.get_namespace(ns_levels).await?;
        let table_uuid = Uuid::new_v4();
        let location = req.location.clone().unwrap_or_else(||
            egret_storage::table_base_location("s3://placeholder", ns_levels, &req.name, &table_uuid));
        let md = create_table_metadata(&req, table_uuid, location.clone())?;
        let mloc = self.write_metadata_json(&location, 0, &md).await?;
        self.store.insert_table(ns.id, &req.name, table_uuid, &md, &mloc, &location).await?;
        Ok(LoadTableResult { metadata_location: Some(mloc), metadata: Arc::new(md), config: None })
    }

    pub async fn load_table(&self, ns_levels: &[String], name: &str)
        -> Result<LoadTableResult, CatalogError> {
        let ns = self.store.get_namespace(ns_levels).await?;
        let row = self.store.load_table(ns.id, name).await?;
        Ok(LoadTableResult { metadata_location: Some(row.metadata_location),
            metadata: Arc::new(row.metadata), config: None })
    }

    pub async fn commit_table(&self, ns_levels: &[String], name: &str, req: CommitTableRequest)
        -> Result<LoadTableResult, CatalogError> {
        let ns = self.store.get_namespace(ns_levels).await?;
        let row = self.store.load_table(ns.id, name).await?;
        let new_md = apply_commit(row.metadata.clone(), Some(&row.metadata_location),
            &req.requirements, req.updates)?;
        let mloc = self.write_metadata_json(&row.location, row.current_version + 1, &new_md).await?;
        self.store.cas_commit(row.id, row.current_version, &new_md, &mloc).await?;
        Ok(LoadTableResult { metadata_location: Some(mloc), metadata: Arc::new(new_md), config: None })
    }
}
```
Update `service/mod.rs` to add `pub mod service; pub use service::CatalogService;`. Note the `config` field is filled in Task 9 (signer keys) — left `None` here, set when the HTTP layer knows the request base URL.

- [ ] **Step 3: DB+MinIO-gated view test** (`store_views.rs`) — insert/load/list/drop a view metadata blob:
```rust
//! DB-gated view-store tests. Self-skip without DATABASE_URL.
use egret_catalog::service::CatalogStore;
use egret_metastore::Db;
use std::collections::HashMap;

async fn store() -> Option<CatalogStore> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(CatalogStore::new(db))
}

#[tokio::test]
async fn insert_load_drop_view() {
    let Some(s) = store().await else { eprintln!("skip"); return; };
    let levels = vec![format!("t_{}", uuid::Uuid::new_v4().simple())];
    let ns = s.create_namespace(&levels, &HashMap::new()).await.unwrap();
    let md = serde_json::json!({ "view-uuid": uuid::Uuid::new_v4().to_string() });
    s.insert_view(ns.id, "v", &md, "s3://wh/ns/v/metadata/0.json", "s3://wh/ns/v").await.unwrap();
    assert_eq!(s.load_view(ns.id, "v").await.unwrap().name, "v");
    assert_eq!(s.list_views(ns.id).await.unwrap(), vec!["v".to_string()]);
    s.drop_view(ns.id, "v").await.unwrap();
    assert!(s.load_view(ns.id, "v").await.is_err());
    s.drop_namespace(&levels).await.unwrap();
}
```

- [ ] **Step 4: Run the view tests**

Run: `DATABASE_URL=... cargo test -p egret-catalog store_views`
Expected: PASS with DB.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-catalog
git commit -m "feat(catalog): view store methods + CatalogService orchestration (metadata.json writes)"
```

---

## Task 9: HTTP layer — handlers, error mapping, auth, `iceberg_router()`

**Files:**
- Create: `crates/egret-catalog/src/api/mod.rs`, `crates/egret-catalog/src/api/config.rs`, `crates/egret-catalog/src/api/namespaces.rs`, `crates/egret-catalog/src/api/tables.rs`, `crates/egret-catalog/src/api/views.rs`, `crates/egret-catalog/src/api/sign.rs`
- Create: `crates/egret-catalog/src/auth.rs`, `crates/egret-catalog/src/http_error.rs`, `crates/egret-catalog/src/routes.rs`
- Modify: `crates/egret-catalog/src/lib.rs`
- Test: `crates/egret-catalog/tests/http_roundtrip.rs`

- [ ] **Step 1: HTTP error mapping** (`CatalogError` → Iceberg `ErrorModel` JSON + status)

`crates/egret-catalog/src/http_error.rs`:
```rust
use crate::CatalogError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use egret_iceberg_ext::error::{ErrorModel, IcebergErrorResponse};

impl IntoResponse for CatalogError {
    fn into_response(self) -> Response {
        let (status, typ) = match &self {
            CatalogError::NotFound(_) => (StatusCode::NOT_FOUND, "NoSuchObjectException"),
            CatalogError::AlreadyExists(_) => (StatusCode::CONFLICT, "AlreadyExistsException"),
            CatalogError::Conflict(_) => (StatusCode::CONFLICT, "CommitFailedException"),
            CatalogError::NamespaceNotEmpty(_) => (StatusCode::CONFLICT, "NamespaceNotEmptyException"),
            CatalogError::BadRequest(_) => (StatusCode::BAD_REQUEST, "BadRequestException"),
            CatalogError::Db(_) | CatalogError::Storage(_) =>
                (StatusCode::INTERNAL_SERVER_ERROR, "InternalServerError"),
        };
        let body = IcebergErrorResponse {
            error: ErrorModel::new(self.to_string(), typ, status.as_u16()),
        };
        (status, Json(body)).into_response()
    }
}
```

- [ ] **Step 2: Bearer-token auth middleware**

`crates/egret-catalog/src/auth.rs`:
```rust
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use crate::routes::CatalogState;

/// Constant-time-ish bearer check against the configured catalog token.
pub async fn require_bearer(State(state): State<CatalogState>, req: Request<axum::body::Body>, next: Next)
    -> Result<Response, StatusCode> {
    let token = req.headers().get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    match token {
        Some(t) if constant_time_eq(t.as_bytes(), state.auth_token.as_bytes()) => Ok(next.run(req).await),
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) { diff |= x ^ y; }
    diff == 0
}
```

- [ ] **Step 3: `CatalogState` + `iceberg_router()`** wiring all handlers + the auth layer

`crates/egret-catalog/src/routes.rs`:
```rust
use crate::auth::require_bearer;
use crate::service::CatalogService;
use axum::routing::{get, post};
use axum::Router;
use egret_storage::StorageConfig;
use std::sync::Arc;

#[derive(Clone)]
pub struct CatalogState {
    pub service: CatalogService,
    pub auth_token: Arc<str>,
    pub warehouse: Arc<str>,
    pub storage_cfg: Arc<StorageConfig>,
    /// Base public URL of the catalog, e.g. `http://localhost:8080/catalog`, for signer.uri.
    pub base_url: Arc<str>,
}

pub fn iceberg_router(state: CatalogState) -> Router {
    Router::new()
        .route("/v1/config", get(crate::api::config::get_config))
        .route("/v1/namespaces", get(crate::api::namespaces::list).post(crate::api::namespaces::create))
        .route("/v1/namespaces/{ns}", get(crate::api::namespaces::load)
            .head(crate::api::namespaces::head).delete(crate::api::namespaces::drop))
        .route("/v1/namespaces/{ns}/properties", post(crate::api::namespaces::update_properties))
        .route("/v1/namespaces/{ns}/tables", get(crate::api::tables::list).post(crate::api::tables::create))
        .route("/v1/namespaces/{ns}/tables/{table}", get(crate::api::tables::load)
            .head(crate::api::tables::head).post(crate::api::tables::commit).delete(crate::api::tables::drop))
        .route("/v1/namespaces/{ns}/tables/{table}/metrics", post(crate::api::tables::metrics))
        .route("/v1/namespaces/{ns}/register", post(crate::api::tables::register))
        .route("/v1/tables/rename", post(crate::api::tables::rename))
        .route("/v1/namespaces/{ns}/views", get(crate::api::views::list).post(crate::api::views::create))
        .route("/v1/namespaces/{ns}/views/{view}", get(crate::api::views::load)
            .head(crate::api::views::head).delete(crate::api::views::drop))
        .route("/v1/aws/s3/sign", post(crate::api::sign::sign))
        .layer(axum::middleware::from_fn_with_state(state.clone(), require_bearer))
        .with_state(state)
}
```
Add `pub mod api; pub mod auth; pub mod http_error; pub mod routes;` to `lib.rs`, and `pub use routes::{iceberg_router, CatalogState};`.

- [ ] **Step 4: Implement the handlers.** Namespace path segments arrive as a single `{ns}` with multipart namespaces encoded using the unit separator `\u{1F}` (Iceberg REST convention) or `.`; decode with a helper `parse_namespace(&str) -> Vec<String>` that splits on `\u{1F}`. Each handler extracts `State<CatalogState>`, parses params, calls `state.service`, and returns `Json`.

`crates/egret-catalog/src/api/config.rs`:
```rust
use crate::routes::CatalogState;
use axum::extract::State;
use axum::Json;
use egret_iceberg_ext::catalog::CatalogConfig;
use std::collections::HashMap;

pub async fn get_config(State(s): State<CatalogState>) -> Json<CatalogConfig> {
    let signer_uri = format!("{}/", s.base_url); // clients append the sign path
    let overrides = HashMap::from([
        ("warehouse".to_string(), s.warehouse.to_string()),
        ("s3.remote-signing-enabled".to_string(), "true".to_string()),
        ("s3.signer".to_string(), "S3V4RestSigner".to_string()),
        ("s3.signer.uri".to_string(), signer_uri.clone()),
        ("signer.uri".to_string(), signer_uri),
        ("s3.region".to_string(), s.storage_cfg.region.clone()),
        ("s3.path-style-access".to_string(), s.storage_cfg.path_style.to_string()),
    ]);
    Json(CatalogConfig { overrides, defaults: HashMap::new(), endpoints: vec![] })
}
```

`crates/egret-catalog/src/api/namespaces.rs`, `tables.rs`, `views.rs` follow the same pattern. Example (`tables.rs` create + commit + load):
```rust
use crate::routes::CatalogState;
use crate::CatalogError;
use axum::extract::{Path, State};
use axum::Json;
use egret_iceberg_ext::catalog::{CommitTableRequest, CreateTableRequest, LoadTableResult};

pub fn parse_namespace(ns: &str) -> Vec<String> {
    ns.split('\u{1F}').flat_map(|s| s.split('.')).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

pub async fn create(State(s): State<CatalogState>, Path(ns): Path<String>, Json(req): Json<CreateTableRequest>)
    -> Result<Json<LoadTableResult>, CatalogError> {
    let levels = parse_namespace(&ns);
    Ok(Json(s.service.create_table(&levels, req).await?))
}
pub async fn load(State(s): State<CatalogState>, Path((ns, table)): Path<(String, String)>)
    -> Result<Json<LoadTableResult>, CatalogError> {
    Ok(Json(s.service.load_table(&parse_namespace(&ns), &table).await?))
}
pub async fn commit(State(s): State<CatalogState>, Path((ns, table)): Path<(String, String)>, Json(req): Json<CommitTableRequest>)
    -> Result<Json<LoadTableResult>, CatalogError> {
    Ok(Json(s.service.commit_table(&parse_namespace(&ns), &table, req).await?))
}
pub async fn list(State(s): State<CatalogState>, Path(ns): Path<String>) -> Result<Json<egret_iceberg_ext::catalog::ListTablesResponse>, CatalogError> {
    let levels = parse_namespace(&ns);
    let nsrow = s.service.store.get_namespace(&levels).await?;
    let names = s.service.store.list_tables(nsrow.id).await?;
    let identifiers = names.into_iter().map(|n| egret_iceberg_ext::TableIdent::new(
        egret_iceberg_ext::NamespaceIdent::from_vec(levels.clone()).unwrap(), n)).collect();
    Ok(Json(egret_iceberg_ext::catalog::ListTablesResponse { next_page_token: None, identifiers }))
}
pub async fn head(State(s): State<CatalogState>, Path((ns, table)): Path<(String, String)>) -> Result<axum::http::StatusCode, CatalogError> {
    let nsrow = s.service.store.get_namespace(&parse_namespace(&ns)).await?;
    s.service.store.load_table(nsrow.id, &table).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}
pub async fn drop(State(s): State<CatalogState>, Path((ns, table)): Path<(String, String)>) -> Result<axum::http::StatusCode, CatalogError> {
    let nsrow = s.service.store.get_namespace(&parse_namespace(&ns)).await?;
    s.service.store.drop_table(nsrow.id, &table).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}
pub async fn metrics(Json(_v): Json<serde_json::Value>) -> axum::http::StatusCode { axum::http::StatusCode::NO_CONTENT }
pub async fn rename(State(s): State<CatalogState>, Json(req): Json<egret_iceberg_ext::catalog::RenameTableRequest>) -> Result<axum::http::StatusCode, CatalogError> {
    let src_ns = s.service.store.get_namespace(req.source.namespace.as_ref()).await?;
    let dst_ns = s.service.store.get_namespace(req.destination.namespace.as_ref()).await?;
    s.service.store.rename_table(src_ns.id, &req.source.name, dst_ns.id, &req.destination.name).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}
pub async fn register(State(s): State<CatalogState>, Path(ns): Path<String>, Json(req): Json<egret_iceberg_ext::catalog::RegisterTableRequest>) -> Result<Json<LoadTableResult>, CatalogError> {
    // Read the existing metadata.json from storage, then insert a row pointing at it.
    let levels = parse_namespace(&ns);
    let nsrow = s.service.store.get_namespace(&levels).await?;
    let bytes = s.service.storage.get(&req.metadata_location).await?;
    let md: iceberg::spec::TableMetadata = serde_json::from_slice(&bytes).map_err(|e| CatalogError::BadRequest(e.to_string()))?;
    let table_uuid = md.uuid();
    let location = md.location().to_string();
    s.service.store.insert_table(nsrow.id, &req.name, table_uuid, &md, &req.metadata_location, &location).await?;
    Ok(Json(LoadTableResult { metadata_location: Some(req.metadata_location), metadata: std::sync::Arc::new(md), config: None }))
}
```
(Confirm `NamespaceIdent::from_vec`, `NamespaceIdent::as_ref()`, `TableIdent::new`, `TableMetadata::uuid()`, `TableMetadata::location()` names against the pinned rev; these are the standard iceberg-rust accessors. Namespace/view handlers mirror these shapes.)

`crates/egret-catalog/src/api/sign.rs`:
```rust
use crate::routes::CatalogState;
use crate::CatalogError;
use axum::extract::State;
use axum::Json;
use egret_iceberg_ext::sign::{S3SignRequest as ExtReq, S3SignResponse as ExtResp};

pub async fn sign(State(s): State<CatalogState>, Json(req): Json<ExtReq>) -> Result<Json<ExtResp>, CatalogError> {
    let storage_req = egret_storage::signer::S3SignRequest {
        region: req.region, uri: req.uri, method: req.method, headers: req.headers, body: req.body };
    let r = egret_storage::signer::sign_s3_request(&s.storage_cfg, &storage_req).await?;
    Ok(Json(ExtResp { uri: r.uri, headers: r.headers }))
}
```

`crates/egret-catalog/src/api/mod.rs`:
```rust
pub mod config;
pub mod namespaces;
pub mod tables;
pub mod views;
pub mod sign;
```

- [ ] **Step 5: HTTP round-trip test (DB + MinIO gated)** using `tower::ServiceExt::oneshot`

`crates/egret-catalog/tests/http_roundtrip.rs`: build a `CatalogState` (real `CatalogStore` + an `S3Store` when `EGRET_TEST_S3_ENDPOINT` is set; otherwise skip), then drive requests: create namespace → create table → load table → 401 without bearer. Assert statuses + that load returns the table's metadata. Gate on both `DATABASE_URL` and `EGRET_TEST_S3_ENDPOINT`.

- [ ] **Step 6: Run the HTTP test + clippy**

Run: `DATABASE_URL=... EGRET_TEST_S3_ENDPOINT=http://localhost:9000 cargo test -p egret-catalog http_roundtrip`
Run: `cargo clippy -p egret-catalog --all-targets`
Expected: PASS / no warnings (fix any).

- [ ] **Step 7: Commit**

```bash
git add crates/egret-catalog
git commit -m "feat(catalog): HTTP handlers + auth + error mapping + iceberg_router()"
```

---

## Task 10: Config `[catalog]` + `[storage]`, server wiring, bin wiring

**Files:**
- Modify: `crates/egret-config/src/lib.rs` (add `CatalogConfig` + `StorageConfig` structs, `Default`, `validate()`, tests)
- Modify: `crates/egret-server/src/lib.rs` (`AppState.catalog`, mount `/catalog`)
- Modify: `crates/egret-bin/src/main.rs` (build catalog state from config when `catalog.enabled`)
- Modify: `crates/egret-server/Cargo.toml`, `crates/egret-bin/Cargo.toml` (add `egret-catalog`, `egret-storage` deps)

- [ ] **Step 1: Add config structs**

In `crates/egret-config/src/lib.rs` add (note: name the catalog struct `CatalogSettings` to avoid colliding with `egret_iceberg_ext::catalog::CatalogConfig`):
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogSettings {
    pub enabled: bool,
    pub warehouse: String,
    pub auth_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageSettings {
    pub endpoint: Option<String>,
    pub region: String,
    pub bucket: String,
    pub access_key: String,
    pub secret_key: String,
    pub path_style: bool,
}
```
Add `pub catalog: CatalogSettings,` and `pub storage: StorageSettings,` to `Config`, plus defaults:
```rust
catalog: CatalogSettings { enabled: false, warehouse: "s3://warehouse".into(), auth_token: String::new() },
storage: StorageSettings { endpoint: Some("http://localhost:9000".into()), region: "us-east-1".into(),
    bucket: "warehouse".into(), access_key: String::new(), secret_key: String::new(), path_style: true },
```
Extend `validate()`:
```rust
if self.catalog.enabled {
    if self.catalog.warehouse.is_empty() || self.catalog.auth_token.is_empty() {
        return Err("catalog.enabled requires warehouse and auth_token".into());
    }
    if self.storage.access_key.is_empty() || self.storage.secret_key.is_empty() || self.storage.bucket.is_empty() {
        return Err("catalog.enabled requires storage access_key/secret_key/bucket".into());
    }
}
```

- [ ] **Step 2: Config unit tests** (append to the config tests module):
```rust
#[test]
fn catalog_disabled_by_default_validates() {
    let cfg = Config::default();
    assert!(!cfg.catalog.enabled);
    assert!(cfg.validate().is_ok());
}
#[test]
fn catalog_enabled_requires_token_and_creds() {
    let mut cfg = Config::default();
    cfg.catalog.enabled = true;
    assert!(cfg.validate().is_err());
    cfg.catalog.auth_token = "t".into();
    cfg.storage.access_key = "a".into();
    cfg.storage.secret_key = "s".into();
    assert!(cfg.validate().is_ok());
}
```

- [ ] **Step 3: Run config tests**

Run: `cargo test -p egret-config`
Expected: PASS (new + existing).

- [ ] **Step 4: Wire into `egret-server`** — add `catalog: Option<egret_catalog::CatalogState>` to `AppState`, set it to `None` in `without_db()`, and mount in `build_router`:
```rust
    if let Some(catalog) = state.catalog.clone() {
        router = router.merge(
            Router::new().nest("/catalog", egret_catalog::iceberg_router(catalog)),
        );
    }
```
Add `egret-catalog.workspace = true` to `crates/egret-server/Cargo.toml`. The `iceberg_router` already carries its own `CatalogState`, so the outer `.with_state(state)` is unaffected (nest a fully-stated router).

- [ ] **Step 5: Wire into `egret-bin`** — when `cfg.catalog.enabled`, build the storage + service + state:
```rust
    let catalog = if cfg.catalog.enabled {
        let storage_cfg = egret_storage::StorageConfig {
            endpoint: cfg.storage.endpoint.clone(), region: cfg.storage.region.clone(),
            bucket: cfg.storage.bucket.clone(), access_key: cfg.storage.access_key.clone(),
            secret_key: cfg.storage.secret_key.clone(), path_style: cfg.storage.path_style };
        let store = egret_catalog::service::CatalogStore::new(db.clone());
        let storage = std::sync::Arc::new(egret_storage::S3Store::new(&storage_cfg).await);
        let service = egret_catalog::service::CatalogService { store, storage };
        let base_url = format!("http://{}:{}/catalog", cfg.server.host, cfg.server.port);
        Some(egret_catalog::CatalogState {
            service, auth_token: cfg.catalog.auth_token.clone().into(),
            warehouse: cfg.catalog.warehouse.clone().into(),
            storage_cfg: std::sync::Arc::new(storage_cfg), base_url: base_url.into() })
    } else { None };
```
and pass `catalog` into the `AppState { … }` constructor. Add `egret-catalog.workspace = true` and `egret-storage.workspace = true` to `crates/egret-bin/Cargo.toml`. (Check the exact `AppState` construction site in `egret-bin/src/main.rs` and add the field.)

- [ ] **Step 6: Build the whole workspace + server unit tests**

Run: `cargo build --workspace`
Run: `cargo test -p egret-server`
Expected: compiles; health tests still PASS (catalog `None` in `without_db()`).

- [ ] **Step 7: Commit**

```bash
git add crates/egret-config crates/egret-server crates/egret-bin
git commit -m "feat(catalog): config [catalog]/[storage], server mount /catalog, bin wiring"
```

---

## Task 11: pyiceberg real-client end-to-end gate

**Files:**
- Create: `deploy/` MinIO bucket bootstrap if missing (check `deploy/docker-compose.dev.yml` — it should already have MinIO from Phase 0a; add a `warehouse` bucket via an init step or document `mc mb`).
- Create: `tests/pyiceberg/conftest.py`, `tests/pyiceberg/test_catalog_e2e.py`, `tests/pyiceberg/requirements.txt`
- Create: `crates/egret-catalog/tests/pyiceberg_gate.rs` (Rust wrapper that shells out to pytest; self-skips without `EGRET_TEST_E2E=1`)

- [ ] **Step 1: pyiceberg test script**

`tests/pyiceberg/requirements.txt`:
```text
pyiceberg[pyarrow]==0.8.1
pytest==8.3.3
```
`tests/pyiceberg/test_catalog_e2e.py`:
```python
import os, uuid
import pyarrow as pa
from pyiceberg.catalog.rest import RestCatalog
from pyiceberg.schema import Schema
from pyiceberg.types import NestedField, LongType, StringType

def catalog():
    return RestCatalog("egret", **{
        "uri": os.environ["EGRET_CATALOG_URI"],          # http://localhost:8080/catalog
        "token": os.environ["EGRET_CATALOG_TOKEN"],
        "s3.endpoint": os.environ["EGRET_TEST_S3_ENDPOINT"],
        "s3.access-key-id": os.environ.get("EGRET_TEST_S3_KEY", "minioadmin"),
        "s3.secret-access-key": os.environ.get("EGRET_TEST_S3_SECRET", "minioadmin"),
        "s3.path-style-access": "true",
    })

def test_create_append_load_roundtrip():
    cat = catalog()
    ns = f"e2e_{uuid.uuid4().hex[:8]}"
    cat.create_namespace(ns)
    schema = Schema(
        NestedField(1, "id", LongType(), required=True),
        NestedField(2, "name", StringType(), required=False),
    )
    tbl = cat.create_table(f"{ns}.orders", schema=schema)
    data = pa.table({"id": [1, 2, 3], "name": ["a", "b", "c"]})
    tbl.append(data)
    loaded = cat.load_table(f"{ns}.orders")
    scanned = loaded.scan().to_arrow()
    assert scanned.num_rows == 3
    assert set(scanned.column("id").to_pylist()) == {1, 2, 3}
```
`tests/pyiceberg/conftest.py`: empty (or env validation).

- [ ] **Step 2: Rust gate wrapper** that boots nothing itself (assumes `just up` provides Postgres+MinIO and the egret server is running) and runs pytest only when `EGRET_TEST_E2E=1`:

`crates/egret-catalog/tests/pyiceberg_gate.rs`:
```rust
//! End-to-end gate: drives a real pyiceberg client against a running Egret catalog.
//! Skips unless EGRET_TEST_E2E=1. Requires: Postgres, MinIO (bucket `warehouse`),
//! the egret server running with [catalog].enabled=true, and python+pyiceberg installed.
use std::process::Command;

#[test]
fn pyiceberg_create_append_load() {
    if std::env::var("EGRET_TEST_E2E").ok().as_deref() != Some("1") {
        eprintln!("EGRET_TEST_E2E != 1 — skipping pyiceberg e2e");
        return;
    }
    let status = Command::new("python3")
        .args(["-m", "pytest", "-q", "tests/pyiceberg/test_catalog_e2e.py"])
        .current_dir(env!("CARGO_MANIFEST_DIR").to_string() + "/../..")
        .status().expect("run pytest");
    assert!(status.success(), "pyiceberg e2e failed");
}
```

- [ ] **Step 3: Document the e2e runbook** in the plan's sibling or `tests/pyiceberg/README.md`:
```text
# pyiceberg e2e
1. just up            # Postgres + MinIO
2. mc alias set local http://localhost:9000 minioadmin minioadmin && mc mb -p local/warehouse
3. configure egret.toml: [catalog].enabled=true, auth_token=devtoken, [storage] creds=minioadmin
4. cargo run -p egret-bin -- serve         # in another shell
5. pip install -r tests/pyiceberg/requirements.txt
6. EGRET_TEST_E2E=1 EGRET_CATALOG_URI=http://localhost:8080/catalog EGRET_CATALOG_TOKEN=devtoken \
   EGRET_TEST_S3_ENDPOINT=http://localhost:9000 \
   cargo test -p egret-catalog pyiceberg_gate
```

- [ ] **Step 4: Run the gate (skips by default)**

Run: `cargo test -p egret-catalog pyiceberg_gate`
Expected: prints "skipping" + PASS. With the full runbook + `EGRET_TEST_E2E=1`, the pyiceberg round-trip passes (this is the milestone proof).

- [ ] **Step 5: Commit**

```bash
git add tests/pyiceberg crates/egret-catalog/tests/pyiceberg_gate.rs
git commit -m "test(catalog): pyiceberg real-client e2e gate (create/append/load round-trip)"
```

---

## Task 12: Final workspace verification

- [ ] **Step 1: Full build + clippy + test (offline-safe)**

Run: `cargo build --workspace && cargo clippy --workspace --all-targets && cargo test --workspace`
Expected: builds, no clippy warnings, all tests PASS (DB/MinIO/e2e tests self-skip without their env).

- [ ] **Step 2: With infra up, run the gated tiers**

Run: `just up` then
`DATABASE_URL=postgres://egret:egret@localhost:5432/egret EGRET_TEST_S3_ENDPOINT=http://localhost:9000 cargo test --workspace`
Expected: catalog DB + MinIO tests execute and PASS. Then run the pyiceberg runbook (Task 11 Step 3) and confirm green.

- [ ] **Step 3: Format**

Run: `cargo fmt --all` then commit any formatting.

- [ ] **Step 4: Final commit**

```bash
git add -A
git commit -m "chore(catalog): fmt + final Phase 2a verification"
```

---

## Self-review notes (coverage map spec → tasks)

- Spec §3 crates → Task 0–9 (storage T1–2, iceberg-ext T3, catalog T5–9).
- Spec §4 REST surface → Task 9 (`iceberg_router` covers config/namespace/table/view/sign; metrics no-op; register; rename).
- Spec §5 commit/CAS → Task 6 (`cas_commit`) + Task 7 (`apply_commit`) + Task 8 (`commit_table` orchestration).
- Spec §6 remote signing → Task 2 (`sign_s3_request`) + Task 9 (`/v1/aws/s3/sign` handler + `/v1/config` signer keys).
- Spec §7 auth/config → Task 9 (`require_bearer`) + Task 10 (`[catalog]`/`[storage]` + validate + wiring).
- Spec §8 data model → Task 4 (`0006_catalog.sql`).
- Spec §8.1 performance → hot columns separated (Task 4 schema) + JSONB load (Task 6); cache is a documented follow-up, not in scope.
- Spec §9 testing → unit (T1,2,3,7), DB-gated (T5,6,8), HTTP (T9), pyiceberg e2e (T11), final (T12).

**Known verification points for the implementer (pinned `iceberg` rev `bb70173`):** confirm exact names of `TableMetadataBuilder::new` / `new_from_metadata` / `assign_uuid` / `build()`, `TableUpdate::apply`, `TableRequirement::check`, `TableMetadata::{uuid,location,properties,current_snapshot_id}`, `NamespaceIdent::{from_vec,as_ref}`, `TableIdent::new`, `Schema::builder`, `NestedField::required`, `FormatVersion::V2`. All are used by lakekeeper at the same rev (see `crates/lakekeeper/src/server/commit_tables.rs`, `create_table.rs`); cross-check there if a signature differs.
