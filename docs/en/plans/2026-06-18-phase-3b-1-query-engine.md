# Phase 3b-1 — Query Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Execute SQL against a workspace's granted Trino/StarRocks database asynchronously — background tokio job, status persisted in Postgres, full result streamed to object store as JSONL, results fetched by REST polling, with row-cap/timeout/cancel guards.

**Architecture:** One new crate `egret-engine`. A `Connector` trait (+ `ConnectorFactory`) with Trino (REST statement protocol) and StarRocks (`mysql_async`) impls; a `QueryStore` (Postgres `queries` + JSONL result I/O via `egret-storage`); a `JobManager` (spawns a task per query, in-memory cancel registry, persists lifecycle); REST routes mounted under `/api/ws`. Follows the Phase 3a (`egret-database`) conventions exactly.

**Tech Stack:** Rust 2021 (MSRV 1.85), axum 0.8, sqlx 0.8 (runtime queries — NO `query!` macros), `reqwest` 0.12 (Trino), `mysql_async` 0.34 (StarRocks, `prefer_socket(false)`), `tokio` + `tokio-util` (`CancellationToken`), `async-trait`, `serde_json`.

---

## Conventions every task MUST follow

1. **Runtime sqlx only** — `query_as::<_,Row>`, `query_scalar::<_,T>`, `query().bind()`. NEVER `query!`/`query_as!` macros.
2. **DB-gated tests** self-skip without `DATABASE_URL` (live Postgres at `postgres://egret:egret@localhost:5432/egret`):
   ```rust
   let Some(x) = setup().await else { eprintln!("skip"); return; };
   ```
3. **Engine-gated tests** self-skip unless `EGRET_TEST_TRINO_URL` (`http://localhost:8090`) / `EGRET_TEST_STARROCKS_URL` (`mysql://root@localhost:9030`) is set. The dev engines are up via `deploy/docker-compose.dev.yml`.
4. **Error → HTTP** via `impl IntoResponse` (`DbApiError` pattern in `crates/egret-database/src/error.rs`). Internal DB/storage errors map to 500; never leak driver internals.
5. **Credential hygiene:** the DB secret travels only in connector request headers / connection opts — never in a response, log, or the stored SQL.
6. **Commits:** plain `git commit` (configured identity — never `--author`). Conventional Commits. `cargo fmt --all` before every commit; keep `cargo clippy --workspace --all-targets -- -D warnings` clean.

## Reference facts (verified against the codebase)

- `egret_storage::ObjectStore` trait: `async fn put(&self, uri:&str, bytes:Vec<u8>)`, `get(&self,uri:&str)->Vec<u8>`, `delete(&self,uri:&str)`. **`uri` is a full `s3://<bucket>/<key>`** (S3Store splits on `s3://`). `StorageError::{NotFound(String), S3(String), ...}`. Build the result URI as `s3://{bucket}/{results_prefix}/{id}.jsonl`.
- `egret_storage::{S3Store, StorageConfig}` — `S3Store::new(&StorageConfig).await`. The `StorageConfig` is already built in `egret-bin::serve` for the catalog (same bucket).
- `egret_database::DatabaseRepo`: `get(id:Uuid)->Result<Database,DatabaseError>`, `open_secret(id:Uuid)->Result<Option<String>,DatabaseError>`, `list_for_workspace(ws:Uuid)->Result<Vec<Database>,DatabaseError>`.
- `egret_database::model::{Engine, ConnOptions, Database}` (module is `pub`). `Database { id:Uuid, name:String, engine:Engine, host:String, port:i32, options:ConnOptions, has_secret:bool, status:String }`. `Engine::{Trino,StarRocks}` with `.as_str()`. `ConnOptions { user:Option<String>, http_scheme:Option<String>, catalog:Option<String>, database:Option<String> }`.
- `egret_metastore::Db { pub pool: sqlx::PgPool }`; `Db::connect(url,max)`, `db.migrate()`. Migrator embeds `../../migrations` → `0009_engine.sql` auto-picked-up.
- `egret_core::{Error, UserId, WorkspaceId, WorkspaceContext, Principal}`. `WorkspaceContext { workspace_id:WorkspaceId, principal:Principal, workspace_role:String }`; `Principal { user_id:UserId, platform_roles:Vec<String>, ... }`.
- RBAC: `egret_workspace::context::WsContext` extractor (resolves `{workspace}` slug → `WorkspaceContext`; needs `SessionService`+`WorkspaceRepo`+`Authorizer` as `FromRef` on the router state). `egret_workspace::rbac::Authorizer::require_workspace(&ctx, "perm") -> egret_core::Result<()>` (Err = Forbidden; platform_admin bypasses).
- `egret_auth::session::SessionService`, `egret_auth::csrf::csrf_layer`.
- Existing Trino/StarRocks protocol slices to copy the approach from: `crates/egret-database/src/tester.rs` (`test_trino`, `test_starrocks` with `prefer_socket(false)`).
- `AppState` in `crates/egret-server/src/lib.rs`; `serve()` in `crates/egret-bin/src/main.rs`.

## File structure

```text
Cargo.toml                                # MODIFY: workspace deps (egret-engine, tokio-util)
migrations/0009_engine.sql                # CREATE
crates/egret-config/src/lib.rs            # MODIFY: EngineConfig + validate + tests
crates/egret-engine/
  Cargo.toml                              # CREATE
  src/lib.rs                              # CREATE
  src/model.rs                            # CREATE: QueryStatus, Column, QueryResult, Limits, DTOs
  src/error.rs                            # CREATE: EngineError + EngineApiError
  src/connector/mod.rs                    # CREATE: Connector + ConnectorFactory + value mapping
  src/connector/trino.rs                  # CREATE
  src/connector/starrocks.rs              # CREATE
  src/store.rs                            # CREATE: QueryStore + JSONL result I/O
  src/job.rs                              # CREATE: JobManager
  src/routes.rs                           # CREATE: EngineState + routers
  tests/store.rs                          # CREATE: DB-gated
  tests/job.rs                            # CREATE: DB-gated (fake connector)
  tests/http.rs                           # CREATE: DB-gated
  tests/connector_trino.rs                # CREATE: engine-gated
  tests/connector_starrocks.rs            # CREATE: engine-gated
crates/egret-server/src/lib.rs            # MODIFY: AppState.engine + mount
crates/egret-bin/src/main.rs              # MODIFY: build EngineState
```

---

## Task 1: Scaffold `egret-engine` crate + workspace deps

**Files:** Modify `Cargo.toml`; Create `crates/egret-engine/Cargo.toml`, `crates/egret-engine/src/lib.rs`.

- [ ] **Step 1: Workspace deps** — in root `Cargo.toml` `[workspace.dependencies]` add:
```toml
egret-engine = { path = "crates/egret-engine" }
tokio-util = "0.7"
```

- [ ] **Step 2: Create `crates/egret-engine/Cargo.toml`**
```toml
[package]
name = "egret-engine"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
egret-core.workspace = true
egret-config.workspace = true
egret-metastore.workspace = true
egret-auth.workspace = true
egret-workspace.workspace = true
egret-database.workspace = true
egret-storage.workspace = true
axum.workspace = true
sqlx.workspace = true
serde.workspace = true
serde_json.workspace = true
uuid.workspace = true
chrono.workspace = true
thiserror.workspace = true
anyhow.workspace = true
tracing.workspace = true
reqwest.workspace = true
mysql_async.workspace = true
tokio.workspace = true
tokio-util.workspace = true
async-trait.workspace = true

[dev-dependencies]
tower.workspace = true
http-body-util = "0.1"
url = { workspace = true }
```

- [ ] **Step 3: Create `crates/egret-engine/src/lib.rs` (stub — modules filled by later tasks)**
```rust
//! Async query engine: connectors (Trino/StarRocks), job manager, result store.

pub mod connector;
pub mod model;
mod error;
mod job;
mod routes;
mod store;

pub use error::{EngineApiError, EngineError};
pub use job::JobManager;
pub use routes::{engine_router, EngineState};
pub use store::QueryStore;
```
> **Implementer note:** create each referenced module file (`model.rs`, `error.rs`, `connector/mod.rs`, `connector/trino.rs`, `connector/starrocks.rs`, `store.rs`, `job.rs`, `routes.rs`) with a `//! placeholder` line so `cargo build` succeeds, BUT to keep `lib.rs`'s `pub use` lines resolving you may instead defer the `pub use` lines: comment them out in this task and have each later task add its own re-export. Pick whichever keeps `cargo build --workspace` GREEN with zero errors. Recommended: in Task 1, `lib.rs` declares modules only (no `pub use`), and the module files are empty placeholders. Later tasks add the `pub use` lines as they implement each item. `connector/mod.rs` must exist for the `connector/` subdir module to resolve.

- [ ] **Step 4: Verify** — `cargo build --workspace` → PASS.
- [ ] **Step 5: Commit**
```bash
git add Cargo.toml crates/egret-engine
git commit -m "feat(engine): scaffold egret-engine crate"
```

---

## Task 2: `model.rs` + `error.rs`

**Files:** Create `crates/egret-engine/src/model.rs`, `crates/egret-engine/src/error.rs`; update `lib.rs` re-exports.

- [ ] **Step 1: Write the failing unit tests** (append to `model.rs`)
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn status_roundtrips() {
        assert_eq!(QueryStatus::Finished.as_str(), "finished");
        assert_eq!(QueryStatus::parse("cancelled"), Some(QueryStatus::Cancelled));
        assert_eq!(QueryStatus::parse("nope"), None);
    }

    #[test]
    fn column_serializes_type_key() {
        let c = Column { name: "n".into(), ty: "bigint".into() };
        assert_eq!(serde_json::to_value(&c).unwrap(), json!({"name":"n","type":"bigint"}));
    }

    #[test]
    fn jsonl_row_roundtrips() {
        let row = vec![json!(1), json!("x"), json!(null)];
        let line = serde_json::to_string(&row).unwrap();
        let back: Vec<serde_json::Value> = serde_json::from_str(&line).unwrap();
        assert_eq!(back, row);
    }
}
```

- [ ] **Step 2: Run** `cargo test -p egret-engine model` → compile FAIL.

- [ ] **Step 3: Implement `crates/egret-engine/src/model.rs`** (above the tests)
```rust
//! Engine domain types + request/response DTOs.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QueryStatus {
    Queued,
    Running,
    Finished,
    Failed,
    Cancelled,
}

impl QueryStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            QueryStatus::Queued => "queued",
            QueryStatus::Running => "running",
            QueryStatus::Finished => "finished",
            QueryStatus::Failed => "failed",
            QueryStatus::Cancelled => "cancelled",
        }
    }
    pub fn parse(s: &str) -> Option<QueryStatus> {
        Some(match s {
            "queued" => QueryStatus::Queued,
            "running" => QueryStatus::Running,
            "finished" => QueryStatus::Finished,
            "failed" => QueryStatus::Failed,
            "cancelled" => QueryStatus::Cancelled,
            _ => return None,
        })
    }
}

/// A result column: a name and an engine type label.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

/// A fully-collected query result held in memory (bounded by `max_rows`).
#[derive(Debug, Clone)]
pub struct QueryResult {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<serde_json::Value>>,
    pub row_count: u64,
    pub bytes_scanned: Option<u64>,
    pub truncated: bool,
}

/// Execution guards.
#[derive(Debug, Clone)]
pub struct Limits {
    pub max_rows: usize,
    pub timeout: Duration,
}

// ── Request / response DTOs ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SubmitQuery {
    pub database_id: Uuid,
    pub sql: String,
}

#[derive(Debug, Serialize)]
pub struct SubmitAck {
    pub id: String,
    pub status: QueryStatus,
}

/// Full status view (preview inline; full result via the results endpoint).
#[derive(Debug, Serialize)]
pub struct QueryStatusDto {
    pub id: String,
    pub database_id: String,
    pub status: QueryStatus,
    pub columns: Option<serde_json::Value>,
    pub preview: Option<serde_json::Value>,
    pub row_count: Option<i64>,
    pub bytes_scanned: Option<i64>,
    pub truncated: bool,
    pub error: Option<String>,
}

/// History list item (no result bodies).
#[derive(Debug, Serialize)]
pub struct QueryListItem {
    pub id: String,
    pub database_id: String,
    pub status: QueryStatus,
    pub sql: String,
    pub row_count: Option<i64>,
    pub submitted_at: chrono::DateTime<chrono::Utc>,
}

/// A page of the full result.
#[derive(Debug, Serialize)]
pub struct ResultsPage {
    pub columns: serde_json::Value,
    pub rows: Vec<serde_json::Value>,
    pub page: usize,
    pub page_size: usize,
    pub row_count: i64,
    pub truncated: bool,
}
```

- [ ] **Step 4: Implement `crates/egret-engine/src/error.rs`**
```rust
//! Engine error + HTTP mapping.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("forbidden")]
    Forbidden,
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("connect: {0}")]
    Connect(String),
    #[error("engine: {0}")]
    Engine(String),
    #[error("timeout")]
    Timeout,
    #[error("cancelled")]
    Cancelled,
    #[error("limit: {0}")]
    Limit(String),
    #[error(transparent)]
    Storage(#[from] egret_storage::StorageError),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

impl From<egret_database::DatabaseError> for EngineError {
    fn from(e: egret_database::DatabaseError) -> Self {
        match e {
            egret_database::DatabaseError::NotFound(s) => EngineError::NotFound(s),
            other => EngineError::Engine(other.to_string()),
        }
    }
}

/// HTTP-facing error: an authz/validation failure or a domain error.
pub enum EngineApiError {
    Auth(egret_core::Error),
    Domain(EngineError),
}

impl From<egret_core::Error> for EngineApiError {
    fn from(e: egret_core::Error) -> Self {
        EngineApiError::Auth(e)
    }
}
impl From<EngineError> for EngineApiError {
    fn from(e: EngineError) -> Self {
        EngineApiError::Domain(e)
    }
}

impl IntoResponse for EngineApiError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            EngineApiError::Auth(e) => match e {
                egret_core::Error::NotFound(_) => (StatusCode::NOT_FOUND, "not found".to_string()),
                egret_core::Error::InvalidInput(m) => (StatusCode::BAD_REQUEST, m),
                egret_core::Error::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".to_string()),
                egret_core::Error::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
                egret_core::Error::Conflict(m) => (StatusCode::CONFLICT, m),
                egret_core::Error::TooManyRequests => (StatusCode::TOO_MANY_REQUESTS, "too many requests".to_string()),
                egret_core::Error::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal error".to_string()),
            },
            EngineApiError::Domain(e) => match &e {
                EngineError::NotFound(_) => (StatusCode::NOT_FOUND, e.to_string()),
                EngineError::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
                EngineError::BadRequest(_) | EngineError::Limit(_) => (StatusCode::BAD_REQUEST, e.to_string()),
                EngineError::Storage(_) | EngineError::Db(_) => {
                    (StatusCode::INTERNAL_SERVER_ERROR, "internal error".to_string())
                }
                // Connect/Engine/Timeout/Cancelled are normally surfaced via the query
                // row, not a request error; if they reach here, treat as 400.
                _ => (StatusCode::BAD_REQUEST, e.to_string()),
            },
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}
```

- [ ] **Step 5: Update `lib.rs` re-exports** — ensure `pub mod model;`, `pub use error::{EngineApiError, EngineError};` are present (add now if you deferred them in Task 1).

- [ ] **Step 6: Run** `cargo test -p egret-engine model` → PASS; `cargo build -p egret-engine` clean.

- [ ] **Step 7: Commit**
```bash
cargo fmt --all
git add crates/egret-engine/src/model.rs crates/egret-engine/src/error.rs crates/egret-engine/src/lib.rs
git commit -m "feat(engine): query model + error types"
```

---

## Task 3: `Connector` trait + `ConnectorFactory` + Trino connector

**Files:** Create `crates/egret-engine/src/connector/mod.rs`, `crates/egret-engine/src/connector/trino.rs`, `crates/egret-engine/tests/connector_trino.rs`.

- [ ] **Step 1: Write the failing engine-gated test** `crates/egret-engine/tests/connector_trino.rs`
```rust
//! Engine-gated Trino connector test. Self-skip unless EGRET_TEST_TRINO_URL is set.
use egret_database::model::{ConnOptions, Engine};
use egret_engine::connector::{build_connector, ConnSpec};
use egret_engine::model::Limits;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn trino_select_1() {
    let Some(base) = std::env::var("EGRET_TEST_TRINO_URL").ok() else {
        eprintln!("skip");
        return;
    };
    let url = url::Url::parse(&base).unwrap();
    let spec = ConnSpec {
        engine: Engine::Trino,
        host: url.host_str().unwrap().to_string(),
        port: url.port().unwrap_or(8080) as i32,
        options: ConnOptions {
            user: Some("admin".into()),
            http_scheme: Some(url.scheme().to_string()),
            ..Default::default()
        },
        secret: None,
    };
    let c = build_connector(&spec);
    let limits = Limits { max_rows: 100, timeout: Duration::from_secs(30) };
    let r = c.execute("SELECT 1 AS one", &limits, CancellationToken::new()).await.unwrap();
    assert_eq!(r.row_count, 1);
    assert_eq!(r.columns.len(), 1);
    assert_eq!(r.rows[0][0], serde_json::json!(1));
    assert!(!r.truncated);
}

#[tokio::test]
async fn trino_row_cap_truncates() {
    let Some(base) = std::env::var("EGRET_TEST_TRINO_URL").ok() else {
        eprintln!("skip");
        return;
    };
    let url = url::Url::parse(&base).unwrap();
    let spec = ConnSpec {
        engine: Engine::Trino,
        host: url.host_str().unwrap().to_string(),
        port: url.port().unwrap_or(8080) as i32,
        options: ConnOptions { user: Some("admin".into()), http_scheme: Some(url.scheme().to_string()), ..Default::default() },
        secret: None,
    };
    let c = build_connector(&spec);
    let limits = Limits { max_rows: 5, timeout: Duration::from_secs(30) };
    // generate 100 rows; expect cap at 5 + truncated
    let r = c.execute("SELECT * FROM UNNEST(SEQUENCE(1,100)) AS t(n)", &limits, CancellationToken::new()).await.unwrap();
    assert_eq!(r.row_count, 5);
    assert!(r.truncated);
}
```

- [ ] **Step 2: Run** `cargo test -p egret-engine --test connector_trino` → compile FAIL.

- [ ] **Step 3: Implement `crates/egret-engine/src/connector/mod.rs`**
```rust
//! Connector trait + per-engine construction + value mapping.

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use egret_database::model::{ConnOptions, Database, Engine};

use crate::error::EngineError;
use crate::model::{Limits, QueryResult};

mod starrocks;
mod trino;

/// Everything needed to open a connection for one query.
#[derive(Debug, Clone)]
pub struct ConnSpec {
    pub engine: Engine,
    pub host: String,
    pub port: i32,
    pub options: ConnOptions,
    pub secret: Option<String>,
}

impl ConnSpec {
    /// Build from a resolved registry `Database` + its opened secret.
    pub fn from_database(db: &Database, secret: Option<String>) -> Self {
        ConnSpec {
            engine: db.engine,
            host: db.host.clone(),
            port: db.port,
            options: db.options.clone(),
            secret,
        }
    }
}

#[async_trait]
pub trait Connector: Send + Sync {
    /// Run `sql`, honoring `limits` and `cancel`. Fetches up to `max_rows + 1` rows;
    /// if the extra row appears, sets `truncated` and drops it. On a triggered
    /// `cancel`, best-effort aborts the engine query and returns `EngineError::Cancelled`.
    async fn execute(
        &self,
        sql: &str,
        limits: &Limits,
        cancel: CancellationToken,
    ) -> Result<QueryResult, EngineError>;
}

/// Construct the right connector for a spec.
pub fn build_connector(spec: &ConnSpec) -> Box<dyn Connector> {
    match spec.engine {
        Engine::Trino => Box::new(trino::TrinoConnector::new(spec)),
        Engine::StarRocks => Box::new(starrocks::StarRocksConnector::new(spec)),
    }
}

/// A production `ConnectorFactory` resolves a database id → connector via the registry.
/// Defined here so `JobManager` can be tested with a fake factory.
#[async_trait]
pub trait ConnectorFactory: Send + Sync {
    async fn build(&self, db: &Database, secret: Option<String>) -> Result<Box<dyn Connector>, EngineError>;
}

/// The default factory: build directly from the registry row.
pub struct DefaultConnectorFactory;

#[async_trait]
impl ConnectorFactory for DefaultConnectorFactory {
    async fn build(&self, db: &Database, secret: Option<String>) -> Result<Box<dyn Connector>, EngineError> {
        Ok(build_connector(&ConnSpec::from_database(db, secret)))
    }
}
```

- [ ] **Step 4: Implement `crates/egret-engine/src/connector/trino.rs`**
```rust
//! Trino connector — REST statement protocol with row-cap, timeout, cancellation.

use async_trait::async_trait;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use super::{ConnSpec, Connector};
use crate::error::EngineError;
use crate::model::{Column, Limits, QueryResult};

pub struct TrinoConnector {
    base_url: String,
    user: String,
    secret: Option<String>,
}

impl TrinoConnector {
    pub fn new(spec: &ConnSpec) -> Self {
        let scheme = spec.options.http_scheme.as_deref().unwrap_or("http");
        let user = spec.options.user.clone().unwrap_or_else(|| "egret".into());
        TrinoConnector {
            base_url: format!("{scheme}://{}:{}", spec.host, spec.port),
            user,
            secret: spec.secret.clone(),
        }
    }
}

#[derive(Deserialize)]
struct TrinoResp {
    #[serde(default, rename = "nextUri")]
    next_uri: Option<String>,
    #[serde(default)]
    error: Option<TrinoError>,
    #[serde(default)]
    columns: Option<Vec<TrinoColumn>>,
    #[serde(default)]
    data: Option<Vec<Vec<serde_json::Value>>>,
    #[serde(default)]
    stats: Option<TrinoStats>,
}
#[derive(Deserialize)]
struct TrinoColumn { name: String, #[serde(rename = "type")] ty: String }
#[derive(Deserialize)]
struct TrinoError { message: String }
#[derive(Deserialize)]
struct TrinoStats { #[serde(default, rename = "processedBytes")] processed_bytes: Option<u64> }

#[async_trait]
impl Connector for TrinoConnector {
    async fn execute(
        &self,
        sql: &str,
        limits: &Limits,
        cancel: CancellationToken,
    ) -> Result<QueryResult, EngineError> {
        let client = reqwest::Client::builder()
            .timeout(limits.timeout)
            .build()
            .map_err(|e| EngineError::Connect(e.to_string()))?;

        // Engine-native run-time cap via a session header (outer tokio timeout is added by JobManager).
        let session = format!("query_max_run_time={}s", limits.timeout.as_secs().max(1));
        let mut req = client
            .post(format!("{}/v1/statement", self.base_url))
            .header("X-Trino-User", &self.user)
            .header("X-Trino-Session", session)
            .body(sql.to_string());
        if let Some(s) = &self.secret {
            req = req.basic_auth(&self.user, Some(s));
        }

        let cap = limits.max_rows;
        let mut columns: Vec<Column> = Vec::new();
        let mut rows: Vec<Vec<serde_json::Value>> = Vec::new();
        let mut bytes_scanned: Option<u64> = None;

        let mut resp: TrinoResp = send(req).await?;
        loop {
            if let Some(err) = resp.error.take() {
                return Err(EngineError::Engine(err.message));
            }
            if columns.is_empty() {
                if let Some(cols) = resp.columns.take() {
                    columns = cols.into_iter().map(|c| Column { name: c.name, ty: c.ty }).collect();
                }
            }
            if let Some(st) = &resp.stats {
                if st.processed_bytes.is_some() {
                    bytes_scanned = st.processed_bytes;
                }
            }
            if let Some(data) = resp.data.take() {
                for r in data {
                    if rows.len() <= cap {
                        rows.push(r);
                    }
                    if rows.len() > cap {
                        break; // we have the +1 overflow row
                    }
                }
            }
            // cancellation or overflow → abort the statement
            let next = resp.next_uri.take();
            if cancel.is_cancelled() {
                if let Some(uri) = &next {
                    let _ = client.delete(uri).header("X-Trino-User", &self.user).send().await;
                }
                return Err(EngineError::Cancelled);
            }
            if rows.len() > cap {
                if let Some(uri) = &next {
                    let _ = client.delete(uri).header("X-Trino-User", &self.user).send().await;
                }
                break;
            }
            match next {
                None => break,
                Some(uri) => {
                    let mut g = client.get(&uri).header("X-Trino-User", &self.user);
                    if let Some(s) = &self.secret {
                        g = g.basic_auth(&self.user, Some(s));
                    }
                    resp = send(g).await?;
                }
            }
        }

        let truncated = rows.len() > cap;
        if truncated {
            rows.truncate(cap);
        }
        Ok(QueryResult {
            columns,
            row_count: rows.len() as u64,
            rows,
            bytes_scanned,
            truncated,
        })
    }
}

async fn send(req: reqwest::RequestBuilder) -> Result<TrinoResp, EngineError> {
    let resp = req.send().await.map_err(|e| EngineError::Connect(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(EngineError::Engine(format!("http {}", resp.status())));
    }
    resp.json::<TrinoResp>().await.map_err(|e| EngineError::Engine(format!("decode: {e}")))
}
```

- [ ] **Step 5: Add a placeholder `connector/starrocks.rs`** so `mod starrocks;` resolves (Task 4 fills it):
```rust
//! StarRocks connector (implemented in Task 4).
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use super::{ConnSpec, Connector};
use crate::error::EngineError;
use crate::model::{Limits, QueryResult};

pub struct StarRocksConnector;
impl StarRocksConnector {
    pub fn new(_spec: &ConnSpec) -> Self { StarRocksConnector }
}
#[async_trait]
impl Connector for StarRocksConnector {
    async fn execute(&self, _sql: &str, _limits: &Limits, _cancel: CancellationToken) -> Result<QueryResult, EngineError> {
        Err(EngineError::Engine("starrocks connector not yet implemented".into()))
    }
}
```

- [ ] **Step 6: Wire `lib.rs`** — ensure `pub mod connector;` is present (it is). The connector module re-exports nothing at crate root; tests use `egret_engine::connector::{build_connector, ConnSpec}` which are `pub` in `connector/mod.rs`.

- [ ] **Step 7: Run** — `cargo build -p egret-engine` clean; `EGRET_TEST_TRINO_URL=http://localhost:8090 cargo test -p egret-engine --test connector_trino` → 2 passed. Without the env var → skip.

- [ ] **Step 8: Commit**
```bash
cargo fmt --all
git add crates/egret-engine/src/connector crates/egret-engine/src/lib.rs crates/egret-engine/tests/connector_trino.rs
git commit -m "feat(engine): Connector trait + Trino connector"
```

---

## Task 4: StarRocks connector

**Files:** Replace `crates/egret-engine/src/connector/starrocks.rs`; Create `crates/egret-engine/tests/connector_starrocks.rs`.

- [ ] **Step 1: Write the failing engine-gated test** `crates/egret-engine/tests/connector_starrocks.rs`
```rust
//! Engine-gated StarRocks connector test. Self-skip unless EGRET_TEST_STARROCKS_URL is set.
use egret_database::model::{ConnOptions, Engine};
use egret_engine::connector::{build_connector, ConnSpec};
use egret_engine::model::Limits;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn spec(host: &str, port: i32) -> ConnSpec {
    ConnSpec {
        engine: Engine::StarRocks,
        host: host.into(),
        port,
        options: ConnOptions { user: Some("root".into()), ..Default::default() },
        secret: None,
    }
}

fn parse() -> Option<(String, i32)> {
    let base = std::env::var("EGRET_TEST_STARROCKS_URL").ok()?;
    let u = url::Url::parse(&base).ok()?;
    Some((u.host_str()?.to_string(), u.port().unwrap_or(9030) as i32))
}

#[tokio::test]
async fn starrocks_select_1() {
    let Some((h, p)) = parse() else { eprintln!("skip"); return; };
    let c = build_connector(&spec(&h, p));
    let limits = Limits { max_rows: 100, timeout: Duration::from_secs(30) };
    let r = c.execute("SELECT 1 AS one", &limits, CancellationToken::new()).await.unwrap();
    assert_eq!(r.row_count, 1);
    assert_eq!(r.columns.len(), 1);
    assert_eq!(r.rows[0][0], serde_json::json!(1));
}

#[tokio::test]
async fn starrocks_row_cap_truncates() {
    let Some((h, p)) = parse() else { eprintln!("skip"); return; };
    let c = build_connector(&spec(&h, p));
    let limits = Limits { max_rows: 3, timeout: Duration::from_secs(30) };
    // numbers table generator: 10 rows
    let r = c.execute("SELECT * FROM TABLE(generate_series(1, 10))", &limits, CancellationToken::new()).await.unwrap();
    assert_eq!(r.row_count, 3);
    assert!(r.truncated);
}
```
> **Implementer note:** if `generate_series` is unavailable on the dev StarRocks image, substitute any query that reliably returns >3 rows (e.g. `SELECT number FROM TABLE(generate_series(1,10))`, or a `UNION ALL` of literals). Verify against the live engine and adjust the SQL so the truncation test exercises >max_rows rows. The `import ConnOptions` is via `egret_database::model::ConnOptions` — add the use at the top.

- [ ] **Step 2: Run** `EGRET_TEST_STARROCKS_URL=mysql://root@localhost:9030 cargo test -p egret-engine --test connector_starrocks` → FAIL (stub errors).

- [ ] **Step 3: Implement `crates/egret-engine/src/connector/starrocks.rs`**
```rust
//! StarRocks connector — MySQL protocol (mysql_async) with row-cap + cancellation.

use async_trait::async_trait;
use futures_util::StreamExt;
use mysql_async::prelude::Queryable;
use tokio_util::sync::CancellationToken;

use super::{ConnSpec, Connector};
use crate::error::EngineError;
use crate::model::{Column, Limits, QueryResult};

pub struct StarRocksConnector {
    opts: mysql_async::Opts,
}

impl StarRocksConnector {
    pub fn new(spec: &ConnSpec) -> Self {
        let user = spec.options.user.clone().unwrap_or_else(|| "root".into());
        let mut b = mysql_async::OptsBuilder::default()
            .ip_or_hostname(spec.host.clone())
            .tcp_port(u16::try_from(spec.port).unwrap_or(9030))
            .user(Some(user))
            .prefer_socket(false);
        if let Some(s) = &spec.secret {
            b = b.pass(Some(s.clone()));
        }
        if let Some(db) = &spec.options.database {
            b = b.db_name(Some(db.clone()));
        }
        StarRocksConnector { opts: b.into() }
    }
}

#[async_trait]
impl Connector for StarRocksConnector {
    async fn execute(
        &self,
        sql: &str,
        limits: &Limits,
        cancel: CancellationToken,
    ) -> Result<QueryResult, EngineError> {
        let mut conn = mysql_async::Conn::new(self.opts.clone())
            .await
            .map_err(|e| EngineError::Connect(e.to_string()))?;

        let mut result = conn
            .exec_iter(sql, ())
            .await
            .map_err(|e| EngineError::Engine(e.to_string()))?;

        // Column schema from the active result set.
        let columns: Vec<Column> = result
            .columns()
            .map(|cols| {
                cols.iter()
                    .map(|c| Column { name: c.name_str().to_string(), ty: format!("{:?}", c.column_type()) })
                    .collect()
            })
            .unwrap_or_default();

        let cap = limits.max_rows;
        let mut rows: Vec<Vec<serde_json::Value>> = Vec::new();
        let mut stream = result.stream::<mysql_async::Row>().await
            .map_err(|e| EngineError::Engine(e.to_string()))?
            .ok_or_else(|| EngineError::Engine("no result set".into()))?;

        while let Some(row) = stream.next().await {
            if cancel.is_cancelled() {
                drop(stream);
                let _ = conn.disconnect().await;
                return Err(EngineError::Cancelled);
            }
            let row = row.map_err(|e| EngineError::Engine(e.to_string()))?;
            rows.push(row_to_json(row));
            if rows.len() > cap {
                break; // +1 overflow captured
            }
        }
        drop(stream);
        let _ = conn.disconnect().await;

        let truncated = rows.len() > cap;
        if truncated {
            rows.truncate(cap);
        }
        Ok(QueryResult {
            columns,
            row_count: rows.len() as u64,
            rows,
            bytes_scanned: None,
            truncated,
        })
    }
}

fn row_to_json(row: mysql_async::Row) -> Vec<serde_json::Value> {
    use mysql_async::Value as V;
    (0..row.len())
        .map(|i| match row.as_ref(i) {
            Some(V::NULL) | None => serde_json::Value::Null,
            Some(V::Int(n)) => serde_json::json!(n),
            Some(V::UInt(n)) => serde_json::json!(n),
            Some(V::Float(f)) => serde_json::json!(f),
            Some(V::Double(f)) => serde_json::json!(f),
            Some(V::Bytes(b)) => serde_json::Value::String(String::from_utf8_lossy(b).into_owned()),
            Some(other) => serde_json::Value::String(format!("{:?}", other)),
        })
        .collect()
}
```
> **Implementer note:** add `futures-util = "0.3"` to `egret-engine` `[dependencies]` (workspace doesn't pin it yet — add `futures-util = "0.3"` to root `[workspace.dependencies]` and use `futures-util.workspace = true`, OR add it directly to the crate). Verify the `mysql_async` 0.34 streaming API: `QueryResult::columns()` returns `Option<Arc<[Column]>>`, `stream::<Row>()` yields the row stream; `Row::as_ref(i) -> Option<&Value>`, `Column::name_str()`, `Column::column_type()`. Adjust the exact calls if the 0.34 surface differs (it may be `result.stream_and_drop` or `collect`). The simplest robust fallback: `let rows: Vec<Row> = result.collect().await?;` then cap in memory — use that if streaming-with-cancel proves fiddly, and check `cancel` once before collecting. Keep `prefer_socket(false)` (StarRocks rejects the socket probe).

- [ ] **Step 4: Run** `EGRET_TEST_STARROCKS_URL=mysql://root@localhost:9030 cargo test -p egret-engine --test connector_starrocks` → 2 passed.

- [ ] **Step 5: Commit**
```bash
cargo fmt --all
git add crates/egret-engine/src/connector/starrocks.rs crates/egret-engine/tests/connector_starrocks.rs Cargo.toml crates/egret-engine/Cargo.toml
git commit -m "feat(engine): StarRocks connector"
```

---

## Task 5: Migration `0009` + `QueryStore` + JSONL result I/O

**Files:** Create `migrations/0009_engine.sql`, `crates/egret-engine/src/store.rs`, `crates/egret-engine/tests/store.rs`; update `lib.rs`.

- [ ] **Step 1: Write `migrations/0009_engine.sql`**
```sql
CREATE TABLE queries (
  id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id  uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  database_id   uuid NOT NULL REFERENCES databases(id) ON DELETE CASCADE,
  user_id       uuid REFERENCES users(id),
  sql           text NOT NULL,
  status        text NOT NULL DEFAULT 'queued'
                  CHECK (status IN ('queued','running','finished','failed','cancelled')),
  columns_jsonb jsonb,
  preview_jsonb jsonb,
  row_count     bigint,
  bytes_scanned bigint,
  truncated     boolean NOT NULL DEFAULT false,
  result_ref    text,
  error         text,
  submitted_at  timestamptz NOT NULL DEFAULT now(),
  started_at    timestamptz,
  finished_at   timestamptz
);
CREATE INDEX queries_ws_user_idx ON queries (workspace_id, user_id, submitted_at DESC);

INSERT INTO role_permissions (role_id, permission)
SELECT id, 'query:run' FROM roles WHERE scope = 'workspace' AND name IN ('ws_editor','ws_admin');
```

- [ ] **Step 2: Write the failing DB-gated tests** `crates/egret-engine/tests/store.rs`
```rust
//! DB-gated QueryStore tests. Self-skip without DATABASE_URL.
use egret_engine::model::{Column, QueryResult, QueryStatus};
use egret_engine::QueryStore;
use egret_metastore::Db;
use uuid::Uuid;

async fn setup() -> Option<(QueryStore, Db, Uuid, Uuid)> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    // a throwaway user + workspace + database
    let uid: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, provider) VALUES ($1,'local') RETURNING id")
        .bind(format!("u_{}@t.local", Uuid::new_v4().simple()))
        .fetch_one(&db.pool).await.unwrap();
    let wid: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by) VALUES ($1,'t',$2) RETURNING id")
        .bind(format!("w-{}", Uuid::new_v4().simple())).bind(uid)
        .fetch_one(&db.pool).await.unwrap();
    let did: Uuid = sqlx::query_scalar(
        "INSERT INTO databases (name, engine, host, port) VALUES ($1,'trino','h',8080) RETURNING id")
        .bind(format!("db_{}", Uuid::new_v4().simple()))
        .fetch_one(&db.pool).await.unwrap();
    Some((QueryStore::new(db.clone()), db, wid, did))
}

#[tokio::test]
async fn lifecycle_queued_running_finished() {
    let Some((store, _db, wid, did)) = setup().await else { eprintln!("skip"); return; };
    let uid: Uuid = sqlx::query_scalar("SELECT user_id FROM workspaces LIMIT 0").fetch_optional(&_db.pool).await.unwrap().unwrap_or_else(Uuid::new_v4);
    let id = store.insert(wid, did, Some(uid), "SELECT 1").await.unwrap();
    let q = store.get(id).await.unwrap().unwrap();
    assert_eq!(q.status, QueryStatus::Queued);

    store.set_running(id).await.unwrap();
    assert_eq!(store.get(id).await.unwrap().unwrap().status, QueryStatus::Running);

    let result = QueryResult {
        columns: vec![Column { name: "n".into(), ty: "bigint".into() }],
        rows: vec![vec![serde_json::json!(1)], vec![serde_json::json!(2)]],
        row_count: 2, bytes_scanned: Some(10), truncated: false,
    };
    store.finish(id, &result, "s3://warehouse/query-results/x.jsonl", 1).await.unwrap();
    let q = store.get(id).await.unwrap().unwrap();
    assert_eq!(q.status, QueryStatus::Finished);
    assert_eq!(q.row_count, Some(2));
    assert!(q.result_ref.is_some());
}

#[tokio::test]
async fn fail_and_cancel_set_status() {
    let Some((store, _db, wid, did)) = setup().await else { eprintln!("skip"); return; };
    let id = store.insert(wid, did, None, "SELECT 1").await.unwrap();
    store.fail(id, "boom").await.unwrap();
    let q = store.get(id).await.unwrap().unwrap();
    assert_eq!(q.status, QueryStatus::Failed);
    assert_eq!(q.error.as_deref(), Some("boom"));

    let id2 = store.insert(wid, did, None, "SELECT 1").await.unwrap();
    store.set_cancelled(id2).await.unwrap();
    assert_eq!(store.get(id2).await.unwrap().unwrap().status, QueryStatus::Cancelled);
}

#[tokio::test]
async fn jsonl_serialization_roundtrip() {
    let result = QueryResult {
        columns: vec![Column { name: "n".into(), ty: "bigint".into() }],
        rows: vec![vec![serde_json::json!(1)], vec![serde_json::json!("x")]],
        row_count: 2, bytes_scanned: None, truncated: false,
    };
    let bytes = egret_engine::QueryStore::result_to_jsonl(&result);
    let page = egret_engine::QueryStore::jsonl_slice(&bytes, 0, 10);
    assert_eq!(page.len(), 2);
    assert_eq!(page[0], serde_json::json!([1]));
    assert_eq!(page[1], serde_json::json!(["x"]));
}
```
> **Implementer note:** the `uid` fetch hack in `lifecycle_*` is awkward — simplify: capture the throwaway `uid` from `setup()` by returning it too, or pass `None` for `user_id`. Return `(QueryStore, Db, wid, did, uid)` from `setup()` and use `Some(uid)`. Fix the helper so the test is clean; the point is to exercise insert→running→finish with a real row.

- [ ] **Step 3: Run** `DATABASE_URL=... cargo test -p egret-engine --test store` → compile FAIL.

- [ ] **Step 4: Implement `crates/egret-engine/src/store.rs`**
```rust
//! Postgres `queries` store + JSONL result I/O over an ObjectStore.

use std::sync::Arc;

use egret_metastore::Db;
use egret_storage::ObjectStore;
use uuid::Uuid;

use crate::error::EngineError;
use crate::model::{QueryResult, QueryStatus};

#[derive(Clone)]
pub struct QueryStore {
    db: Db,
}

#[derive(sqlx::FromRow)]
pub struct QueryRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub database_id: Uuid,
    pub user_id: Option<Uuid>,
    pub sql: String,
    pub status: String,
    pub columns_jsonb: Option<serde_json::Value>,
    pub preview_jsonb: Option<serde_json::Value>,
    pub row_count: Option<i64>,
    pub bytes_scanned: Option<i64>,
    pub truncated: bool,
    pub result_ref: Option<String>,
    pub error: Option<String>,
    pub submitted_at: chrono::DateTime<chrono::Utc>,
}

/// A typed view used by the routes.
pub struct Query {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub database_id: Uuid,
    pub user_id: Option<Uuid>,
    pub sql: String,
    pub status: QueryStatus,
    pub columns: Option<serde_json::Value>,
    pub preview: Option<serde_json::Value>,
    pub row_count: Option<i64>,
    pub bytes_scanned: Option<i64>,
    pub truncated: bool,
    pub result_ref: Option<String>,
    pub error: Option<String>,
    pub submitted_at: chrono::DateTime<chrono::Utc>,
}

impl QueryRow {
    fn into_query(self) -> Query {
        let status = QueryStatus::parse(&self.status).unwrap_or(QueryStatus::Failed);
        Query {
            id: self.id, workspace_id: self.workspace_id, database_id: self.database_id,
            user_id: self.user_id, sql: self.sql, status,
            columns: self.columns_jsonb, preview: self.preview_jsonb,
            row_count: self.row_count, bytes_scanned: self.bytes_scanned,
            truncated: self.truncated, result_ref: self.result_ref, error: self.error,
            submitted_at: self.submitted_at,
        }
    }
}

const COLS: &str = "id, workspace_id, database_id, user_id, sql, status, columns_jsonb, \
    preview_jsonb, row_count, bytes_scanned, truncated, result_ref, error, submitted_at";

impl QueryStore {
    pub fn new(db: Db) -> Self { Self { db } }

    pub async fn insert(&self, ws: Uuid, db_id: Uuid, user: Option<Uuid>, sql: &str) -> Result<Uuid, EngineError> {
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO queries (workspace_id, database_id, user_id, sql) \
             VALUES ($1,$2,$3,$4) RETURNING id")
            .bind(ws).bind(db_id).bind(user).bind(sql)
            .fetch_one(&self.db.pool).await?;
        Ok(id)
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<Query>, EngineError> {
        let row = sqlx::query_as::<_, QueryRow>(&format!("SELECT {COLS} FROM queries WHERE id=$1"))
            .bind(id).fetch_optional(&self.db.pool).await?;
        Ok(row.map(QueryRow::into_query))
    }

    pub async fn list_for_user(&self, ws: Uuid, user: Uuid, limit: i64) -> Result<Vec<Query>, EngineError> {
        let rows = sqlx::query_as::<_, QueryRow>(&format!(
            "SELECT {COLS} FROM queries WHERE workspace_id=$1 AND user_id=$2 \
             ORDER BY submitted_at DESC LIMIT $3"))
            .bind(ws).bind(user).bind(limit).fetch_all(&self.db.pool).await?;
        Ok(rows.into_iter().map(QueryRow::into_query).collect())
    }

    pub async fn set_running(&self, id: Uuid) -> Result<(), EngineError> {
        sqlx::query("UPDATE queries SET status='running', started_at=now() WHERE id=$1")
            .bind(id).execute(&self.db.pool).await?;
        Ok(())
    }

    pub async fn set_cancelled(&self, id: Uuid) -> Result<(), EngineError> {
        sqlx::query("UPDATE queries SET status='cancelled', finished_at=now() WHERE id=$1")
            .bind(id).execute(&self.db.pool).await?;
        Ok(())
    }

    pub async fn fail(&self, id: Uuid, error: &str) -> Result<(), EngineError> {
        sqlx::query("UPDATE queries SET status='failed', error=$2, finished_at=now() WHERE id=$1")
            .bind(id).bind(error).execute(&self.db.pool).await?;
        Ok(())
    }

    /// Persist a finished result. `preview_rows` controls the inline preview size.
    pub async fn finish(&self, id: Uuid, result: &QueryResult, result_ref: &str, preview_rows: usize) -> Result<(), EngineError> {
        let columns = serde_json::to_value(&result.columns).unwrap_or(serde_json::json!([]));
        let preview = serde_json::to_value(
            &result.rows.iter().take(preview_rows).collect::<Vec<_>>()
        ).unwrap_or(serde_json::json!([]));
        sqlx::query(
            "UPDATE queries SET status='finished', columns_jsonb=$2, preview_jsonb=$3, \
             row_count=$4, bytes_scanned=$5, truncated=$6, result_ref=$7, finished_at=now() WHERE id=$1")
            .bind(id).bind(columns).bind(preview)
            .bind(result.row_count as i64)
            .bind(result.bytes_scanned.map(|b| b as i64))
            .bind(result.truncated).bind(result_ref)
            .execute(&self.db.pool).await?;
        Ok(())
    }

    // ── JSONL result I/O ─────────────────────────────────────────────────────

    /// Serialize rows as JSONL (one JSON array per line). Columns live in Postgres.
    pub fn result_to_jsonl(result: &QueryResult) -> Vec<u8> {
        let mut buf = String::new();
        for row in &result.rows {
            buf.push_str(&serde_json::to_string(row).unwrap_or_else(|_| "[]".into()));
            buf.push('\n');
        }
        buf.into_bytes()
    }

    /// Slice a page of rows from a JSONL byte buffer (MVP: parse + slice in memory).
    pub fn jsonl_slice(bytes: &[u8], offset: usize, limit: usize) -> Vec<serde_json::Value> {
        String::from_utf8_lossy(bytes)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .skip(offset)
            .take(limit)
            .map(|l| serde_json::from_str(l).unwrap_or(serde_json::Value::Null))
            .collect()
    }

    /// Write the full result to the object store; returns the `s3://…` uri used.
    pub async fn write_result(&self, storage: &Arc<dyn ObjectStore>, uri: &str, result: &QueryResult) -> Result<(), EngineError> {
        storage.put(uri, Self::result_to_jsonl(result)).await?;
        Ok(())
    }

    /// Read a page of the full result from the object store.
    pub async fn read_results(&self, storage: &Arc<dyn ObjectStore>, uri: &str, offset: usize, limit: usize) -> Result<Vec<serde_json::Value>, EngineError> {
        let bytes = storage.get(uri).await?;
        Ok(Self::jsonl_slice(&bytes, offset, limit))
    }
}
```

- [ ] **Step 5: Update `lib.rs`** — ensure `pub use store::QueryStore;` (and `pub use store::Query;` if routes need the typed view — add `pub use store::{Query, QueryStore};`).

- [ ] **Step 6: Run** `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-engine --test store` → all pass (fix the `setup()` helper per the note). `cargo build -p egret-engine` clean.

- [ ] **Step 7: Commit**
```bash
cargo fmt --all
git add migrations/0009_engine.sql crates/egret-engine/src/store.rs crates/egret-engine/src/lib.rs crates/egret-engine/tests/store.rs
git commit -m "feat(engine): 0009 migration + QueryStore with JSONL result I/O"
```

---

## Task 6: `egret-config` `[engine]` + validate

**Files:** Modify `crates/egret-config/src/lib.rs`.

- [ ] **Step 1: Write the failing tests** (into the existing `mod tests`)
```rust
    #[test]
    fn engine_defaults_present_and_validate() {
        let cfg = Config::default();
        assert_eq!(cfg.engine.max_rows, 100_000);
        assert_eq!(cfg.engine.query_timeout_secs, 60);
        assert_eq!(cfg.engine.preview_rows, 100);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn engine_preview_must_not_exceed_max_rows() {
        let mut cfg = Config::default();
        cfg.engine.preview_rows = 200_000; // > max_rows
        assert!(cfg.validate().is_err());
    }
```

- [ ] **Step 2: Run** `cargo test -p egret-config engine_` → compile FAIL.

- [ ] **Step 3: Add `EngineConfig` struct** (next to `SecretsConfig`)
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineConfig {
    pub max_rows: usize,
    pub query_timeout_secs: u64,
    pub preview_rows: usize,
    /// Object-store key prefix for query results (under the storage bucket).
    pub results_prefix: String,
}
```

- [ ] **Step 4: Add the field to `Config`** (after `pub secrets: SecretsConfig,`):
```rust
    pub engine: EngineConfig,
```

- [ ] **Step 5: Add the default** (in `impl Default for Config`, after the `secrets:` block):
```rust
            engine: EngineConfig {
                max_rows: 100_000,
                query_timeout_secs: 60,
                preview_rows: 100,
                results_prefix: "query-results".into(),
            },
```

- [ ] **Step 6: Add validation** (in `validate()`, before the final `Ok(())`):
```rust
        if self.engine.max_rows < 1 || self.engine.query_timeout_secs < 1 {
            return Err("engine.max_rows and engine.query_timeout_secs must be >= 1".into());
        }
        if self.engine.preview_rows > self.engine.max_rows {
            return Err("engine.preview_rows must be <= engine.max_rows".into());
        }
```

- [ ] **Step 7: Run** `cargo test -p egret-config` → all pass. **Step 8: Commit**
```bash
cargo fmt --all
git add crates/egret-config/src/lib.rs
git commit -m "feat(config): add [engine] limits"
```

---

## Task 7: `JobManager` (submit / run / cancel)

**Files:** Create `crates/egret-engine/src/job.rs`, `crates/egret-engine/tests/job.rs`; update `lib.rs`.

- [ ] **Step 1: Write the failing DB-gated tests** `crates/egret-engine/tests/job.rs` (uses a FAKE connector — no real engine needed)
```rust
//! DB-gated JobManager tests with an injected fake connector.
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use egret_database::model::Database;
use egret_engine::connector::{Connector, ConnectorFactory};
use egret_engine::model::{Column, Limits, QueryResult, QueryStatus};
use egret_engine::{JobManager, QueryStore};
use egret_metastore::Db;
use egret_storage::{ObjectStore, StorageError};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

// ---- fakes ----
struct FakeConnector { sleep_ms: u64 }
#[async_trait]
impl Connector for FakeConnector {
    async fn execute(&self, _sql: &str, _limits: &Limits, cancel: CancellationToken) -> Result<QueryResult, egret_engine::EngineError> {
        // cooperative: poll the token while "running"
        for _ in 0..(self.sleep_ms / 10).max(1) {
            if cancel.is_cancelled() { return Err(egret_engine::EngineError::Cancelled); }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok(QueryResult {
            columns: vec![Column { name: "n".into(), ty: "bigint".into() }],
            rows: vec![vec![serde_json::json!(1)]],
            row_count: 1, bytes_scanned: None, truncated: false,
        })
    }
}
struct FakeFactory { sleep_ms: u64 }
#[async_trait]
impl ConnectorFactory for FakeFactory {
    async fn build(&self, _db: &Database, _secret: Option<String>) -> Result<Box<dyn Connector>, egret_engine::EngineError> {
        Ok(Box::new(FakeConnector { sleep_ms: self.sleep_ms }))
    }
}
#[derive(Clone)]
struct MemStore(Arc<std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>>);
#[async_trait]
impl ObjectStore for MemStore {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), StorageError> { self.0.lock().unwrap().insert(uri.into(), bytes); Ok(()) }
    async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError> { self.0.lock().unwrap().get(uri).cloned().ok_or_else(|| StorageError::NotFound(uri.into())) }
    async fn delete(&self, uri: &str) -> Result<(), StorageError> { self.0.lock().unwrap().remove(uri); Ok(()) }
}

async fn setup(sleep_ms: u64) -> Option<(JobManager, QueryStore, Db, Uuid, Uuid)> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let wid: Uuid = sqlx::query_scalar("INSERT INTO workspaces (slug,name) VALUES ($1,'t') RETURNING id")
        .bind(format!("w-{}", Uuid::new_v4().simple())).fetch_one(&db.pool).await.unwrap();
    let did: Uuid = sqlx::query_scalar("INSERT INTO databases (name,engine,host,port) VALUES ($1,'trino','h',8080) RETURNING id")
        .bind(format!("db_{}", Uuid::new_v4().simple())).fetch_one(&db.pool).await.unwrap();
    let store = QueryStore::new(db.clone());
    let storage: Arc<dyn ObjectStore> = Arc::new(MemStore(Default::default()));
    let jobs = JobManager::new(
        store.clone(),
        egret_database::DatabaseRepo::new(db.clone(), egret_secrets::SecretStore::new(db.clone(), None)),
        Arc::new(FakeFactory { sleep_ms }),
        storage,
        "s3://warehouse".into(),
        "query-results".into(),
        Limits { max_rows: 100, timeout: Duration::from_secs(10) },
        100,
    );
    Some((jobs, store, db, wid, did))
}

async fn wait_status(store: &QueryStore, id: Uuid, want: QueryStatus, tries: u32) -> QueryStatus {
    for _ in 0..tries {
        let s = store.get(id).await.unwrap().unwrap().status;
        if s == want { return s; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    store.get(id).await.unwrap().unwrap().status
}

#[tokio::test]
async fn submit_runs_to_finished() {
    let Some((jobs, store, _db, wid, did)) = setup(0).await else { eprintln!("skip"); return; };
    let id = jobs.submit(wid, did, None, "SELECT 1").await.unwrap();
    assert_eq!(wait_status(&store, id, QueryStatus::Finished, 40).await, QueryStatus::Finished);
    let q = store.get(id).await.unwrap().unwrap();
    assert_eq!(q.row_count, Some(1));
    assert!(q.result_ref.is_some());
}

#[tokio::test]
async fn cancel_sets_cancelled() {
    let Some((jobs, store, _db, wid, did)) = setup(2000).await else { eprintln!("skip"); return; };
    let id = jobs.submit(wid, did, None, "SELECT 1").await.unwrap();
    // let it enter running, then cancel
    wait_status(&store, id, QueryStatus::Running, 20).await;
    jobs.cancel(id).await.unwrap();
    assert_eq!(wait_status(&store, id, QueryStatus::Cancelled, 40).await, QueryStatus::Cancelled);
}
```
> **Implementer note:** the `databases` insert here omits `secret_id` (nullable) — fine. `JobManager::new` signature must match this call (store, DatabaseRepo, Arc<dyn ConnectorFactory>, Arc<dyn ObjectStore>, bucket_uri `s3://<bucket>`, results_prefix, Limits, preview_rows). Adjust the test if you choose a config struct instead of positional args, but keep it injectable.

- [ ] **Step 2: Run** `DATABASE_URL=... cargo test -p egret-engine --test job` → compile FAIL.

- [ ] **Step 3: Implement `crates/egret-engine/src/job.rs`**
```rust
//! JobManager: async query execution with a Postgres-persisted lifecycle.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use egret_database::DatabaseRepo;
use egret_storage::ObjectStore;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::connector::ConnectorFactory;
use crate::error::EngineError;
use crate::model::Limits;
use crate::store::QueryStore;

#[derive(Clone)]
pub struct JobManager {
    store: QueryStore,
    databases: DatabaseRepo,
    factory: Arc<dyn ConnectorFactory>,
    storage: Arc<dyn ObjectStore>,
    bucket_uri: Arc<str>,     // e.g. "s3://warehouse"
    results_prefix: Arc<str>, // e.g. "query-results"
    limits: Limits,
    preview_rows: usize,
    cancels: Arc<Mutex<HashMap<Uuid, CancellationToken>>>,
}

impl JobManager {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        store: QueryStore,
        databases: DatabaseRepo,
        factory: Arc<dyn ConnectorFactory>,
        storage: Arc<dyn ObjectStore>,
        bucket_uri: String,
        results_prefix: String,
        limits: Limits,
        preview_rows: usize,
    ) -> Self {
        JobManager {
            store, databases, factory, storage,
            bucket_uri: bucket_uri.into(),
            results_prefix: results_prefix.into(),
            limits, preview_rows,
            cancels: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Insert a queued row, spawn the execution task, return the id.
    pub async fn submit(&self, ws: Uuid, database_id: Uuid, user: Option<Uuid>, sql: &str) -> Result<Uuid, EngineError> {
        let id = self.store.insert(ws, database_id, user, sql).await?;
        let token = CancellationToken::new();
        self.cancels.lock().unwrap().insert(id, token.clone());
        let this = self.clone();
        let sql = sql.to_string();
        tokio::spawn(async move {
            this.run(id, database_id, sql, token).await;
            this.cancels.lock().unwrap().remove(&id);
        });
        Ok(id)
    }

    /// Cancel a running query (best-effort): trigger the token; the connector aborts.
    pub async fn cancel(&self, id: Uuid) -> Result<(), EngineError> {
        if let Some(tok) = self.cancels.lock().unwrap().get(&id).cloned() {
            tok.cancel();
        }
        Ok(())
    }

    async fn run(&self, id: Uuid, database_id: Uuid, sql: String, token: CancellationToken) {
        if let Err(e) = self.store.set_running(id).await {
            tracing::warn!(%id, error=%e, "failed to mark query running");
            return;
        }
        let outcome = self.execute(database_id, &sql, &token).await;
        match outcome {
            Ok(()) => {} // finish() already persisted inside execute()
            Err(EngineError::Cancelled) => {
                let _ = self.store.set_cancelled(id).await;
            }
            Err(e) => {
                let _ = self.store.fail(id, &e.to_string()).await;
            }
        }
        let _ = id;
    }

    /// Resolve the connection, run with the outer timeout, write the result, persist finished.
    async fn execute(&self, database_id: Uuid, sql: &str, token: &CancellationToken) -> Result<(), EngineError> {
        let db = self.databases.get(database_id).await?;        // EngineError: From<DatabaseError>
        let secret = self.databases.open_secret(database_id).await?;
        let connector = self.factory.build(&db, secret).await?;

        let fut = connector.execute(sql, &self.limits, token.clone());
        let result = match tokio::time::timeout(self.limits.timeout, fut).await {
            Ok(r) => r?,                       // Ok(QueryResult) or connector EngineError
            Err(_) => {
                token.cancel();                // signal the connector to abort
                return Err(EngineError::Timeout);
            }
        };

        // We need the query id to name the result object; thread it via execute's caller.
        // (See note: pass `id` into execute() — store it before running.)
        unreachable!("execute is invoked with id in the real impl; see Step 4 note")
    }
}
```
> **Implementer note (IMPORTANT — finalize the `execute` signature):** the result object key needs the query `id`. Change `execute` to take `id: Uuid` and finish there:
> ```rust
> async fn execute(&self, id: Uuid, database_id: Uuid, sql: &str, token: &CancellationToken) -> Result<(), EngineError> {
>     let db = self.databases.get(database_id).await?;
>     let secret = self.databases.open_secret(database_id).await?;
>     let connector = self.factory.build(&db, secret).await?;
>     let fut = connector.execute(sql, &self.limits, token.clone());
>     let result = match tokio::time::timeout(self.limits.timeout, fut).await {
>         Ok(r) => r?,
>         Err(_) => { token.cancel(); return Err(EngineError::Timeout); }
>     };
>     let uri = format!("{}/{}/{}.jsonl", self.bucket_uri, self.results_prefix.trim_matches('/'), id);
>     self.store.write_result(&self.storage, &uri, &result).await?;
>     self.store.finish(id, &result, &uri, self.preview_rows).await?;
>     Ok(())
> }
> ```
> and call `self.execute(id, database_id, &sql, &token)` from `run`. Remove the placeholder `unreachable!`. The `bucket_uri` already includes `s3://<bucket>`, so the final uri is `s3://<bucket>/query-results/<id>.jsonl`.

- [ ] **Step 4: Update `lib.rs`** — `pub use job::JobManager;` (and ensure `pub mod connector;` exposes `Connector`/`ConnectorFactory`/`build_connector`/`ConnSpec`/`DefaultConnectorFactory`).

- [ ] **Step 5: Run** `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-engine --test job` → 2 passed. `cargo build -p egret-engine` clean.

- [ ] **Step 6: Commit**
```bash
cargo fmt --all
git add crates/egret-engine/src/job.rs crates/egret-engine/src/lib.rs crates/egret-engine/tests/job.rs
git commit -m "feat(engine): JobManager async execution + cancellation"
```

---

## Task 8: REST routes + RBAC + ownership + HTTP tests

**Files:** Create `crates/egret-engine/src/routes.rs`, `crates/egret-engine/tests/http.rs`; update `lib.rs`.

- [ ] **Step 1: Write the failing HTTP tests** `crates/egret-engine/tests/http.rs`
```rust
//! Router smoke tests. Unauthenticated requests must be rejected.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_engine::{engine_router, EngineState};
use tower::ServiceExt;

async fn state() -> Option<EngineState> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = egret_metastore::Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(EngineState::for_test(db))  // a test ctor that wires real sub-states + a MemStore + DefaultConnectorFactory
}

#[tokio::test]
async fn submit_requires_auth() {
    let Some(st) = state().await else { eprintln!("skip"); return; };
    let app = engine_router().with_state(st);
    let resp = app.oneshot(
        Request::builder().method("POST").uri("/ws/eng/queries")
            .header("content-type","application/json")
            .body(Body::from(r#"{"database_id":"00000000-0000-0000-0000-000000000000","sql":"SELECT 1"}"#)).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn history_requires_auth() {
    let Some(st) = state().await else { eprintln!("skip"); return; };
    let app = engine_router().with_state(st);
    let resp = app.oneshot(
        Request::builder().uri("/ws/eng/queries").body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
```
> **Implementer note:** add an `EngineState::for_test(db: Db) -> EngineState` constructor under `#[cfg(any(test, feature = "test-support"))]` OR just a plain `pub fn for_test` that builds the sub-states (SessionService, WorkspaceRepo, Authorizer, DatabaseRepo, a `MemStore` ObjectStore, `DefaultConnectorFactory`, default Limits). Keep the WS-path prefix consistent: the router defines routes under `/ws/{workspace}/...` and is mounted at `/api` by the server, giving `/api/ws/...`. The tests here mount the bare router so they hit `/ws/eng/queries`.

- [ ] **Step 2: Run** `DATABASE_URL=... cargo test -p egret-engine --test http` → compile FAIL.

- [ ] **Step 3: Implement `crates/egret-engine/src/routes.rs`**
```rust
//! Workspace-scoped query REST routes.

use std::sync::Arc;

use axum::extract::{FromRef, Path, Query as AxQuery, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use egret_auth::session::SessionService;
use egret_core::Error;
use egret_database::DatabaseRepo;
use egret_storage::ObjectStore;
use egret_workspace::context::WsContext;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use serde::Deserialize;
use uuid::Uuid;

use crate::connector::ConnectorFactory;
use crate::error::EngineApiError;
use crate::job::JobManager;
use crate::model::{Limits, QueryListItem, QueryStatusDto, ResultsPage, SubmitAck, SubmitQuery};
use crate::store::{Query, QueryStore};

#[derive(Clone)]
pub struct EngineState {
    pub store: QueryStore,
    pub jobs: JobManager,
    pub databases: DatabaseRepo,
    pub storage: Arc<dyn ObjectStore>,
    pub bucket_uri: Arc<str>,
    pub results_prefix: Arc<str>,
    pub limits: Limits,
    pub sessions: SessionService,
    pub workspaces: WorkspaceRepo,
    pub authz: Authorizer,
}

// WsContext needs these three as FromRef.
impl FromRef<EngineState> for SessionService { fn from_ref(s: &EngineState) -> Self { s.sessions.clone() } }
impl FromRef<EngineState> for WorkspaceRepo { fn from_ref(s: &EngineState) -> Self { s.workspaces.clone() } }
impl FromRef<EngineState> for Authorizer { fn from_ref(s: &EngineState) -> Self { s.authz.clone() } }

const PERM: &str = "query:run";

pub fn engine_router() -> Router<EngineState> {
    Router::new()
        .route("/ws/{workspace}/queries", post(submit).get(history))
        .route("/ws/{workspace}/queries/{id}", get(status))
        .route("/ws/{workspace}/queries/{id}/results", get(results))
        .route("/ws/{workspace}/queries/{id}/cancel", post(cancel))
}

/// Authz helper: caller must hold `query:run` in this workspace AND the database
/// must be granted to it. Returns the resolved (workspace_id, user_id).
async fn authorize(st: &EngineState, ctx: &egret_core::WorkspaceContext, database_id: Uuid) -> Result<(), EngineApiError> {
    st.authz.require_workspace(ctx, PERM).await.map_err(EngineApiError::from)?;
    let granted = st.databases.list_for_workspace(ctx.workspace_id.0).await
        .map_err(|e| EngineApiError::from(crate::EngineError::from(e)))?;
    if !granted.iter().any(|d| d.id == database_id) {
        return Err(Error::NotFound("database not granted to workspace".into()).into());
    }
    Ok(())
}

async fn submit(
    State(st): State<EngineState>,
    WsContext(ctx): WsContext,
    Json(body): Json<SubmitQuery>,
) -> Result<(axum::http::StatusCode, Json<SubmitAck>), EngineApiError> {
    authorize(&st, &ctx, body.database_id).await?;
    let id = st.jobs.submit(ctx.workspace_id.0, body.database_id, Some(ctx.principal.user_id.0), &body.sql)
        .await.map_err(EngineApiError::from)?;
    Ok((axum::http::StatusCode::ACCEPTED, Json(SubmitAck { id: id.to_string(), status: crate::model::QueryStatus::Queued })))
}

/// Load a query owned by the caller (or platform_admin), scoped to the workspace.
async fn owned(st: &EngineState, ctx: &egret_core::WorkspaceContext, id: Uuid) -> Result<Query, EngineApiError> {
    let q = st.store.get(id).await.map_err(EngineApiError::from)?
        .ok_or_else(|| Error::NotFound("query".into()))?;
    if q.workspace_id != ctx.workspace_id.0 {
        return Err(Error::NotFound("query".into()).into());
    }
    let is_admin = ctx.principal.platform_roles.iter().any(|r| r == "platform_admin");
    if !is_admin && q.user_id != Some(ctx.principal.user_id.0) {
        return Err(Error::Forbidden.into());
    }
    Ok(q)
}

async fn status(State(st): State<EngineState>, WsContext(ctx): WsContext, Path((_ws, id)): Path<(String, Uuid)>)
    -> Result<Json<QueryStatusDto>, EngineApiError> {
    st.authz.require_workspace(&ctx, PERM).await.map_err(EngineApiError::from)?;
    let q = owned(&st, &ctx, id).await?;
    Ok(Json(QueryStatusDto {
        id: q.id.to_string(), database_id: q.database_id.to_string(), status: q.status,
        columns: q.columns, preview: q.preview, row_count: q.row_count,
        bytes_scanned: q.bytes_scanned, truncated: q.truncated, error: q.error,
    }))
}

#[derive(Deserialize)]
struct Page { page: Option<usize>, page_size: Option<usize> }

async fn results(State(st): State<EngineState>, WsContext(ctx): WsContext, Path((_ws, id)): Path<(String, Uuid)>, AxQuery(p): AxQuery<Page>)
    -> Result<Json<ResultsPage>, EngineApiError> {
    st.authz.require_workspace(&ctx, PERM).await.map_err(EngineApiError::from)?;
    let q = owned(&st, &ctx, id).await?;
    let page = p.page.unwrap_or(0);
    let page_size = p.page_size.unwrap_or(100).clamp(1, 1000);
    let rows = match &q.result_ref {
        Some(uri) => st.store.read_results(&st.storage, uri, page * page_size, page_size).await.map_err(EngineApiError::from)?,
        None => vec![],
    };
    Ok(Json(ResultsPage {
        columns: q.columns.unwrap_or(serde_json::json!([])),
        rows, page, page_size,
        row_count: q.row_count.unwrap_or(0), truncated: q.truncated,
    }))
}

async fn cancel(State(st): State<EngineState>, WsContext(ctx): WsContext, Path((_ws, id)): Path<(String, Uuid)>)
    -> Result<Json<serde_json::Value>, EngineApiError> {
    st.authz.require_workspace(&ctx, PERM).await.map_err(EngineApiError::from)?;
    let _q = owned(&st, &ctx, id).await?;
    st.jobs.cancel(id).await.map_err(EngineApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn history(State(st): State<EngineState>, WsContext(ctx): WsContext)
    -> Result<Json<Vec<QueryListItem>>, EngineApiError> {
    st.authz.require_workspace(&ctx, PERM).await.map_err(EngineApiError::from)?;
    let qs = st.store.list_for_user(ctx.workspace_id.0, ctx.principal.user_id.0, 50).await.map_err(EngineApiError::from)?;
    Ok(Json(qs.into_iter().map(|q| QueryListItem {
        id: q.id.to_string(), database_id: q.database_id.to_string(), status: q.status,
        sql: q.sql, row_count: q.row_count, submitted_at: q.submitted_at,
    }).collect()))
}
```
> **Implementer note:** confirm `WorkspaceContext.workspace_id` is `WorkspaceId(Uuid)` → use `.0`; `principal.user_id` is `UserId(Uuid)` → `.0`. Add the `EngineState::for_test` ctor used by `tests/http.rs`. `crate::EngineError` must be reachable (it is — `pub use error::EngineError`).

- [ ] **Step 4: Update `lib.rs`** — `pub use routes::{engine_router, EngineState};`.

- [ ] **Step 5: Run** `cargo clippy -p egret-engine --all-targets -- -D warnings` clean; `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-engine --test http` → 2 passed (401). 

- [ ] **Step 6: Commit**
```bash
cargo fmt --all
git add crates/egret-engine/src/routes.rs crates/egret-engine/src/lib.rs crates/egret-engine/tests/http.rs
git commit -m "feat(engine): REST routes + RBAC + ownership"
```

---

## Task 9: Wire into `egret-server` + `egret-bin`

**Files:** Modify `crates/egret-server/{Cargo.toml,src/lib.rs}`, `crates/egret-bin/{Cargo.toml,src/main.rs}`.

- [ ] **Step 1: Deps** — `crates/egret-server/Cargo.toml` add `egret-engine.workspace = true`. `crates/egret-bin/Cargo.toml` add `egret-engine.workspace = true` (and confirm `egret-storage`, `egret-database`, `egret-secrets` are present — they are from Phase 2/3a).

- [ ] **Step 2: `AppState`** in `crates/egret-server/src/lib.rs` — add after `pub database: ...`:
```rust
    /// Present once the engine is wired (in `egret-bin`); `None` in unit tests.
    pub engine: Option<egret_engine::EngineState>,
```
Add `engine: None,` to `AppState::without_db()`.

- [ ] **Step 3: Mount** in `build_router`, after the `database` block, before `router.with_state(state)`:
```rust
    if let Some(engine) = state.engine.clone() {
        let group = Router::new()
            .nest("/api", egret_engine::engine_router())
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(engine);
        router = router.merge(group);
    }
```
> Note: `engine_router()` defines `/ws/{workspace}/queries…`, so nesting at `/api` yields `/api/ws/{workspace}/queries…` — consistent with the spec. The existing workspace `ws_member_router` is mounted at `/api/ws` with a different state; axum merges by full path, and `/api/ws/{workspace}/queries` vs `/api/ws/{workspace}/members` are distinct leaves. If a path-collision panic occurs at startup, change the engine routes' prefix to `/ws/{workspace}/sql/...` — but they should not collide.

- [ ] **Step 4: Build `EngineState` in `serve()`** (`crates/egret-bin/src/main.rs`), after the `database` block:
```rust
    let engine = {
        let storage_cfg = egret_storage::StorageConfig {
            endpoint: config.storage.endpoint.clone(),
            region: config.storage.region.clone(),
            bucket: config.storage.bucket.clone(),
            access_key: config.storage.access_key.clone(),
            secret_key: config.storage.secret_key.clone(),
            path_style: config.storage.path_style,
        };
        let storage: std::sync::Arc<dyn egret_storage::ObjectStore> =
            std::sync::Arc::new(egret_storage::S3Store::new(&storage_cfg).await?);
        let store = egret_engine::QueryStore::new(db.clone());
        let databases = egret_database::DatabaseRepo::new(
            db.clone(),
            egret_secrets::SecretStore::new(db.clone(), /* crypto */ {
                use base64::Engine as _;
                if config.secrets.master_key.is_empty() { None }
                else {
                    let key = base64::engine::general_purpose::STANDARD
                        .decode(config.secrets.master_key.as_bytes())
                        .map_err(|e| anyhow::anyhow!("secrets.master_key base64: {e}"))?;
                    Some(egret_secrets::Crypto::new(&key).map_err(|e| anyhow::anyhow!(e.to_string()))?)
                }
            }),
        );
        let limits = egret_engine::model::Limits {
            max_rows: config.engine.max_rows,
            timeout: std::time::Duration::from_secs(config.engine.query_timeout_secs),
        };
        let jobs = egret_engine::JobManager::new(
            store.clone(), databases.clone(),
            std::sync::Arc::new(egret_engine::connector::DefaultConnectorFactory),
            storage.clone(),
            format!("s3://{}", config.storage.bucket),
            config.engine.results_prefix.clone(),
            limits.clone(),
            config.engine.preview_rows,
        );
        Some(egret_engine::EngineState {
            store, jobs, databases, storage,
            bucket_uri: format!("s3://{}", config.storage.bucket).into(),
            results_prefix: config.engine.results_prefix.clone().into(),
            limits,
            sessions: sessions.clone(),
            workspaces: WorkspaceRepo::new(db.clone()),
            authz: Authorizer::new(db.clone()),
        })
    };
```
Add `engine,` to the `AppState { .. }` literal.
> **Implementer note:** `DatabaseRepo` and `SecretStore` must be `Clone` (they are). To avoid duplicating the master-key decode (it's also in the `database` block), you may hoist the `crypto`/`SecretStore` build above both blocks and reuse it. Keep it DRY but correct. `Limits` must derive `Clone` (it does in model.rs).

- [ ] **Step 5: Build + test** — `cargo build --workspace`; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo test -p egret-server` (health tests pass with the new `engine: None`).

- [ ] **Step 6: Commit**
```bash
cargo fmt --all
git add crates/egret-server crates/egret-bin
git commit -m "feat(server): mount query engine routes and wire EngineState"
```

---

## Task 10: Final verification

- [ ] **Step 1:** `cargo fmt --all -- --check` clean.
- [ ] **Step 2:** `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] **Step 3:** `cargo test --workspace` (no env) — all green; DB/engine-gated tests self-skip.
- [ ] **Step 4:** DB-backed — `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test --workspace` — `egret-engine` store/job/http tests run for real.
- [ ] **Step 5:** Engine-backed —
```bash
DATABASE_URL=postgres://egret:egret@localhost:5432/egret \
EGRET_TEST_TRINO_URL=http://localhost:8090 \
EGRET_TEST_STARROCKS_URL=mysql://root@localhost:9030 \
cargo test -p egret-engine
```
All connector tests pass against live engines (Trino `SELECT 1` + truncate; StarRocks `SELECT 1` + truncate).
- [ ] **Step 6:** Confirm `egret-metastore`'s migration-count test still passes (it asserts `>= 7`; `0009` keeps it green).
- [ ] **Step 7:** Final whole-feature review, then **superpowers:finishing-a-development-branch** (tests must pass) → push + PR.

---

## Self-review notes (author)

- **Spec coverage:** §3 layout → Tasks 1–8; §4 connectors → Tasks 3–4; §5 migration/data → Task 5; §6 JSONL/limits → Tasks 4,5 (`max_rows+1`, `result_to_jsonl`); §7 lifecycle/cancel → Task 7; §8 RBAC/REST/ownership → Task 8; §9 config/wiring → Tasks 6,9; §11 testing → every task + Task 10.
- **Refinement of spec §4 (logged):** cancellation is **cooperative via `CancellationToken`** passed into `Connector::execute` (the connector aborts + best-effort engine-cancels), rather than a separate `cancel(handle)` trait method. This removes the "who holds the EngineHandle" ambiguity and is simpler/more correct. The `JobManager` triggers the token; on timeout it also triggers the token then records `Timeout`.
- **Type consistency:** `QueryStatus`, `Column{name,ty/"type"}`, `QueryResult`, `Limits`, `ConnSpec`, `Connector`/`ConnectorFactory`, `QueryStore::{insert,get,set_running,fail,set_cancelled,finish,write_result,read_results,result_to_jsonl,jsonl_slice}`, `JobManager::{new,submit,cancel}`, `EngineState{store,jobs,databases,storage,bucket_uri,results_prefix,limits,sessions,workspaces,authz}` are used identically across tasks.
- **No placeholders:** every code step is complete; the four "implementer notes" flag the spots to verify against live APIs (`mysql_async` 0.34 streaming, StarRocks generator SQL, the `execute(id,…)` signature finalization, the `EngineState::for_test` ctor).
- **Deferred (per spec §1):** WS hub (3b-2), hash cache, byte-scanned enforcement, range-read pagination, SQL-level LIMIT, multi-instance cancel reconcile. Not in any task.
