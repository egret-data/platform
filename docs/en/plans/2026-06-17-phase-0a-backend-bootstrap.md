# Phase 0a — Backend Bootstrap & Dev Infrastructure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the Egret Rust workspace skeleton — a binary that boots an axum server with health endpoints, runs Postgres migrations, parses a CLI, loads typed config — plus a local dev stack (Postgres/MinIO/Trino/StarRocks) and CI.

**Architecture:** A Cargo workspace (modular monolith, per [ADR-01](../architecture/2026-06-17-egret-architecture-design.md#2-architecture-decisions-adr-summary)). Phase 0a scaffolds only the foundation crates needed to boot: `egret-core` (error + domain primitives), `egret-config` (typed config), `egret-metastore` (Postgres pool + migrator), `egret-server` (axum router), `egret-bin` (CLI entrypoint). Other crates are added in later phases — YAGNI.

**Tech Stack:** Rust (edition 2021), tokio, axum 0.8, tower/tower-http, sqlx 0.8 (Postgres), figment (config), clap 4 (CLI), thiserror/anyhow, tracing, uuid. Dev infra: Docker Compose, GitHub Actions, just.

**Reference:** [Master Architecture Design](../architecture/2026-06-17-egret-architecture-design.md) §5 (crate map), §9 (data model), §12 (cross-cutting), §16 (roadmap).

---

## File Structure

Files created in this plan and their single responsibility:

```text
Cargo.toml                              # workspace manifest + shared dep versions
rust-toolchain.toml                     # pin toolchain
rustfmt.toml  ·  .clippy.toml           # lint/format config
justfile                                # dev command shortcuts
.env.example                            # env template (DATABASE_URL, etc.)
deploy/docker-compose.dev.yml           # Postgres + MinIO + Trino + StarRocks
deploy/trino/catalog/.gitkeep           # placeholder for engine config (later phases)
.github/workflows/ci.yml                # fmt + clippy + test
migrations/0001_init.sql                # baseline schema (egret_meta)
crates/egret-core/Cargo.toml
crates/egret-core/src/lib.rs            # re-exports
crates/egret-core/src/error.rs          # Error + Result
crates/egret-core/src/ids.rs            # typed IDs (WorkspaceId, UserId)
crates/egret-core/src/context.rs        # Principal, WorkspaceContext
crates/egret-core/src/pagination.rs     # Page, Pagination
crates/egret-config/Cargo.toml
crates/egret-config/src/lib.rs          # Config + load()
crates/egret-metastore/Cargo.toml
crates/egret-metastore/src/lib.rs       # Db pool + MIGRATOR + migrate()
crates/egret-metastore/tests/migrate.rs # DB-backed integration test (gated on DATABASE_URL)
crates/egret-server/Cargo.toml
crates/egret-server/src/lib.rs          # AppState + build_router + health handlers
crates/egret-bin/Cargo.toml
crates/egret-bin/src/main.rs            # clap CLI: serve / migrate / init
crates/egret-bin/src/cli.rs             # Cli/Command definitions (unit-testable)
```

**Note on doc convention:** This is an internal working plan, kept English-only under `docs/en/plans/`. The bilingual (`docs/en` + `docs/vi`) convention applies to architecture/design/ADR docs, not ephemeral plans.

---

### Task 1: Workspace skeleton + tooling + `egret-core` error type

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `rust-toolchain.toml`, `rustfmt.toml`, `.clippy.toml`
- Create: `crates/egret-core/Cargo.toml`, `crates/egret-core/src/lib.rs`, `crates/egret-core/src/error.rs`

- [ ] **Step 1: Create the workspace manifest**

Create `Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = ["crates/*"]

[workspace.package]
edition = "2021"
license = "Apache-2.0"
rust-version = "1.85"

[workspace.dependencies]
egret-core = { path = "crates/egret-core" }
egret-config = { path = "crates/egret-config" }
egret-metastore = { path = "crates/egret-metastore" }
egret-server = { path = "crates/egret-server" }

tokio = { version = "1", features = ["full"] }
axum = "0.8"
tower = { version = "0.5", features = ["util"] }
tower-http = { version = "0.6", features = ["trace", "cors"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
anyhow = "1"
sqlx = { version = "0.8", default-features = false, features = ["runtime-tokio-rustls", "postgres", "macros", "migrate", "uuid", "chrono"] }
figment = { version = "0.10", features = ["toml", "env"] }
clap = { version = "4", features = ["derive", "env"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }

[profile.dev]
opt-level = 0
```

- [ ] **Step 2: Create toolchain & lint config**

Create `rust-toolchain.toml`:

```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
```

> **Note:** `stable` is used (not a fixed `1.82`) because modern dependency versions
> (`uuid` 1.21+, `axum` 0.8, `sqlx` 0.8) require Rust ≥ 1.85. `rust-version = "1.85"`
> documents the MSRV floor.

Create `rustfmt.toml`:

```toml
edition = "2021"
max_width = 100
imports_granularity = "Crate"
group_imports = "StdExternalCrate"
```

Create `.clippy.toml`:

```toml
# project-wide clippy config (extend in later phases)
```

- [ ] **Step 3: Write the failing test for the error type**

Create `crates/egret-core/Cargo.toml`:

```toml
[package]
name = "egret-core"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
thiserror.workspace = true
anyhow.workspace = true
serde.workspace = true
uuid.workspace = true

[dev-dependencies]
serde_json.workspace = true
```

Create `crates/egret-core/src/error.rs`:

```rust
//! Shared error and result types for all Egret crates.

use thiserror::Error;

/// The canonical error type returned across crate boundaries.
#[derive(Debug, Error)]
pub enum Error {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("unauthorized")]
    Unauthorized,

    #[error("forbidden")]
    Forbidden,

    #[error("conflict: {0}")]
    Conflict(String),

    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

/// Convenience alias used throughout the codebase.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_messages_are_stable() {
        assert_eq!(Error::NotFound("user".into()).to_string(), "not found: user");
        assert_eq!(Error::Unauthorized.to_string(), "unauthorized");
        assert_eq!(Error::Forbidden.to_string(), "forbidden");
    }

    #[test]
    fn wraps_anyhow_via_from() {
        let e: Error = anyhow::anyhow!("boom").into();
        assert!(matches!(e, Error::Internal(_)));
    }
}
```

Create `crates/egret-core/src/lib.rs`:

```rust
//! `egret-core` — shared domain primitives and traits for the Egret workspace.

pub mod error;

pub use error::{Error, Result};
```

- [ ] **Step 4: Run the test to verify it fails (not yet compiled)**

Run: `cargo test -p egret-core`
Expected: compiles and PASSES (these are the first tests; if a typo exists it FAILS to compile). The intent: a green baseline for `egret-core`.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml rust-toolchain.toml rustfmt.toml .clippy.toml crates/egret-core
git commit -m "feat(core): scaffold workspace and egret-core error type"
```

---

### Task 2: `egret-core` domain primitives (IDs, context, pagination)

**Files:**
- Create: `crates/egret-core/src/ids.rs`
- Create: `crates/egret-core/src/context.rs`
- Create: `crates/egret-core/src/pagination.rs`
- Modify: `crates/egret-core/src/lib.rs`

- [ ] **Step 1: Write the failing test for typed IDs**

Create `crates/egret-core/src/ids.rs`:

```rust
//! Typed, newtype-wrapped identifiers backed by UUID v4.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! typed_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(pub Uuid);

        impl $name {
            /// Generate a fresh random identifier.
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

typed_id!(WorkspaceId);
typed_id!(UserId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_distinct_types() {
        let a = WorkspaceId::new();
        let b = WorkspaceId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn id_roundtrips_through_json() {
        let id = UserId::new();
        let json = serde_json::to_string(&id).unwrap();
        let back: UserId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }
}
```

- [ ] **Step 2: Write the failing test for the request context**

Create `crates/egret-core/src/context.rs`:

```rust
//! Per-request identity and workspace scoping.
//!
//! Middleware constructs a [`WorkspaceContext`] and injects it into handlers;
//! repositories must filter every workspace-scoped query by `context.workspace_id`.

use crate::ids::{UserId, WorkspaceId};

/// The authenticated caller.
#[derive(Debug, Clone)]
pub struct Principal {
    pub user_id: UserId,
    pub email: String,
    /// Platform-level roles (e.g. `platform_admin`, `data_steward`).
    pub platform_roles: Vec<String>,
}

/// Identity + the workspace the current request operates within.
#[derive(Debug, Clone)]
pub struct WorkspaceContext {
    pub workspace_id: WorkspaceId,
    pub principal: Principal,
    /// Workspace-scoped role (e.g. `ws_admin`, `ws_editor`, `ws_viewer`).
    pub workspace_role: String,
}

impl WorkspaceContext {
    pub fn is_platform_admin(&self) -> bool {
        self.principal
            .platform_roles
            .iter()
            .any(|r| r == "platform_admin")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(roles: Vec<&str>) -> WorkspaceContext {
        WorkspaceContext {
            workspace_id: WorkspaceId::new(),
            principal: Principal {
                user_id: UserId::new(),
                email: "a@b.c".into(),
                platform_roles: roles.into_iter().map(String::from).collect(),
            },
            workspace_role: "ws_editor".into(),
        }
    }

    #[test]
    fn platform_admin_detected() {
        assert!(ctx(vec!["platform_admin"]).is_platform_admin());
        assert!(!ctx(vec!["data_steward"]).is_platform_admin());
    }
}
```

- [ ] **Step 3: Write the failing test for pagination**

Create `crates/egret-core/src/pagination.rs`:

```rust
//! Offset pagination request parameters.

use serde::{Deserialize, Serialize};

/// Pagination request. `page` is 1-based.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Pagination {
    pub page: u32,
    pub per_page: u32,
}

impl Default for Pagination {
    fn default() -> Self {
        Self { page: 1, per_page: 50 }
    }
}

impl Pagination {
    /// Clamp to safe bounds and compute the SQL OFFSET.
    pub fn offset(&self) -> u32 {
        let page = self.page.max(1);
        let per_page = self.per_page.clamp(1, 500);
        (page - 1) * per_page
    }

    /// Clamped page size (the SQL LIMIT).
    pub fn limit(&self) -> u32 {
        self.per_page.clamp(1, 500)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_page_has_zero_offset() {
        let p = Pagination { page: 1, per_page: 20 };
        assert_eq!(p.offset(), 0);
        assert_eq!(p.limit(), 20);
    }

    #[test]
    fn offset_computed_from_page() {
        let p = Pagination { page: 3, per_page: 20 };
        assert_eq!(p.offset(), 40);
    }

    #[test]
    fn per_page_is_clamped() {
        let p = Pagination { page: 1, per_page: 100_000 };
        assert_eq!(p.limit(), 500);
    }

    #[test]
    fn page_zero_treated_as_one() {
        let p = Pagination { page: 0, per_page: 10 };
        assert_eq!(p.offset(), 0);
    }
}
```

- [ ] **Step 4: Wire the modules into the crate**

Replace `crates/egret-core/src/lib.rs` with:

```rust
//! `egret-core` — shared domain primitives and traits for the Egret workspace.

pub mod context;
pub mod error;
pub mod ids;
pub mod pagination;

pub use context::{Principal, WorkspaceContext};
pub use error::{Error, Result};
pub use ids::{UserId, WorkspaceId};
pub use pagination::Pagination;
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p egret-core`
Expected: PASS — all tests in `error`, `ids`, `context`, `pagination`.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-core
git commit -m "feat(core): add typed ids, request context, and pagination"
```

---

### Task 3: `egret-config` typed configuration

**Files:**
- Create: `crates/egret-config/Cargo.toml`
- Create: `crates/egret-config/src/lib.rs`

- [ ] **Step 1: Create the crate manifest**

Create `crates/egret-config/Cargo.toml`:

```toml
[package]
name = "egret-config"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
serde.workspace = true
figment.workspace = true
```

- [ ] **Step 2: Write the failing test for config loading**

Create `crates/egret-config/src/lib.rs`:

```rust
//! Typed, layered configuration: built-in defaults → `egret.toml` → `EGRET_*` env vars.
//!
//! Nested keys use `__` as the env separator, e.g. `EGRET_SERVER__PORT=9000`.

use figment::{
    providers::{Env, Format, Serialized, Toml},
    Figment,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub log: LogConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    /// Postgres connection URL, e.g. `postgres://egret:egret@localhost:5432/egret`.
    pub url: String,
    pub max_connections: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogConfig {
    /// `tracing_subscriber` env-filter directive, e.g. `info,egret=debug`.
    pub filter: String,
    /// Emit JSON logs when true, pretty logs when false.
    pub json: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig { host: "0.0.0.0".into(), port: 8080 },
            database: DatabaseConfig {
                url: "postgres://egret:egret@localhost:5432/egret".into(),
                max_connections: 10,
            },
            log: LogConfig { filter: "info".into(), json: false },
        }
    }
}

impl Config {
    /// Build the layered figment (defaults < file < env). Exposed for testing.
    pub fn figment() -> Figment {
        Figment::from(Serialized::defaults(Config::default()))
            .merge(Toml::file("egret.toml"))
            .merge(Env::prefixed("EGRET_").split("__"))
    }

    /// Load and validate configuration from all layers.
    pub fn load() -> Result<Self, figment::Error> {
        Self::figment().extract()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_load_without_any_env() {
        figment::Jail::expect_with(|_jail| {
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.server.port, 8080);
            assert_eq!(cfg.server.host, "0.0.0.0");
            assert_eq!(cfg.database.max_connections, 10);
            Ok(())
        });
    }

    #[test]
    fn env_overrides_nested_keys() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("EGRET_SERVER__PORT", "9999");
            jail.set_env("EGRET_DATABASE__URL", "postgres://x/y");
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.server.port, 9999);
            assert_eq!(cfg.database.url, "postgres://x/y");
            Ok(())
        });
    }

    #[test]
    fn toml_file_is_merged() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("egret.toml", "[server]\nport = 7000\n")?;
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.server.port, 7000);
            Ok(())
        });
    }
}
```

- [ ] **Step 3: Add the crate to the workspace dependency table**

Edit `Cargo.toml` `[workspace.dependencies]` — confirm `egret-config = { path = "crates/egret-config" }` is present (added in Task 1). No change needed if already there.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p egret-config`
Expected: PASS — `defaults_load_without_any_env`, `env_overrides_nested_keys`, `toml_file_is_merged`.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-config
git commit -m "feat(config): layered typed configuration (defaults/file/env)"
```

---

### Task 4: `egret-metastore` Postgres pool, migrator, baseline migration

**Files:**
- Create: `migrations/0001_init.sql`
- Create: `crates/egret-metastore/Cargo.toml`
- Create: `crates/egret-metastore/src/lib.rs`
- Create: `crates/egret-metastore/tests/migrate.rs`

- [ ] **Step 1: Create the baseline migration**

Create `migrations/0001_init.sql`:

```sql
-- Baseline schema for Egret. Later phases add module tables (see architecture §9).
CREATE TABLE IF NOT EXISTS egret_meta (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO egret_meta (key, value)
VALUES ('schema_origin', 'phase-0a')
ON CONFLICT (key) DO NOTHING;
```

- [ ] **Step 2: Create the crate manifest**

Create `crates/egret-metastore/Cargo.toml`:

```toml
[package]
name = "egret-metastore"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
egret-core.workspace = true
sqlx.workspace = true
tracing.workspace = true

[dev-dependencies]
tokio.workspace = true
```

- [ ] **Step 3: Write the pool wrapper + migrator with a unit test**

Create `crates/egret-metastore/src/lib.rs`:

```rust
//! Postgres access: a pooled connection wrapper and the embedded migrator.

use sqlx::migrate::{MigrateError, Migrator};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

/// All SQL migrations, embedded at compile time from the workspace `migrations/` dir.
pub static MIGRATOR: Migrator = sqlx::migrate!("../../migrations");

/// A thin handle around the shared Postgres pool.
#[derive(Clone)]
pub struct Db {
    pub pool: PgPool,
}

impl Db {
    /// Connect and build the pool.
    pub async fn connect(url: &str, max_connections: u32) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .connect(url)
            .await?;
        Ok(Self { pool })
    }

    /// Run all pending migrations.
    pub async fn migrate(&self) -> Result<(), MigrateError> {
        MIGRATOR.run(&self.pool).await
    }

    /// Cheap liveness probe used by `/readyz`.
    pub async fn ping(&self) -> Result<(), sqlx::Error> {
        sqlx::query("SELECT 1").execute(&self.pool).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrator_embeds_baseline_migration() {
        assert!(
            MIGRATOR.migrations.len() >= 1,
            "expected at least the baseline migration to be embedded"
        );
    }
}
```

- [ ] **Step 4: Run the unit test to verify it passes**

Run: `cargo test -p egret-metastore --lib`
Expected: PASS — `migrator_embeds_baseline_migration` (no database required; the migration is embedded at compile time).

- [ ] **Step 5: Write the DB-backed integration test (gated on `DATABASE_URL`)**

Create `crates/egret-metastore/tests/migrate.rs`:

```rust
//! Integration test: applies migrations against a real Postgres.
//! Skips (passes) when `DATABASE_URL` is unset so `cargo test` stays green offline.
//! In CI / local dev, start Postgres via `deploy/docker-compose.dev.yml`.

use egret_metastore::Db;

#[tokio::test]
async fn applies_migrations_and_seeds_meta() {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL unset — skipping DB integration test");
        return;
    };

    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM egret_meta")
        .fetch_one(&db.pool)
        .await
        .expect("query egret_meta");

    assert!(count >= 1, "baseline row should be present");
}
```

- [ ] **Step 6: Run the integration test offline to confirm it skips cleanly**

Run: `cargo test -p egret-metastore --test migrate`
Expected: PASS (prints the skip message; `DATABASE_URL` is unset in a clean shell).

- [ ] **Step 7: Commit**

```bash
git add migrations crates/egret-metastore
git commit -m "feat(metastore): postgres pool, embedded migrator, baseline migration"
```

---

### Task 5: `egret-server` axum router + health endpoints

**Files:**
- Create: `crates/egret-server/Cargo.toml`
- Create: `crates/egret-server/src/lib.rs`

- [ ] **Step 1: Create the crate manifest**

Create `crates/egret-server/Cargo.toml`:

```toml
[package]
name = "egret-server"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
egret-core.workspace = true
egret-metastore.workspace = true
axum.workspace = true
tower-http.workspace = true
serde_json.workspace = true
tracing.workspace = true

[dev-dependencies]
tokio.workspace = true
tower.workspace = true
http-body-util = "0.1"
```

- [ ] **Step 2: Write the failing test for the health router**

Create `crates/egret-server/src/lib.rs`:

```rust
//! HTTP composition root: shared application state, the router, and health endpoints.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use egret_metastore::Db;

/// Application state shared by all handlers.
#[derive(Clone)]
pub struct AppState {
    /// `None` in unit tests; `Some` once a database is wired in `egret-bin`.
    pub db: Option<Db>,
}

impl AppState {
    /// Build state with no database — used by unit tests and `--no-db` boots.
    pub fn without_db() -> Self {
        Self { db: None }
    }
}

/// Build the root router. Later phases mount feature routers here.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .with_state(state)
}

/// Liveness: the process is up. Always 200.
async fn healthz() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

/// Readiness: dependencies are reachable. 200 when the DB (if any) responds.
async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    match &state.db {
        None => (StatusCode::OK, Json(serde_json::json!({ "status": "ok", "db": "none" }))),
        Some(db) => match db.ping().await {
            Ok(()) => (StatusCode::OK, Json(serde_json::json!({ "status": "ok", "db": "up" }))),
            Err(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "status": "degraded", "db": "down" })),
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt; // for `oneshot`

    #[tokio::test]
    async fn healthz_returns_200_ok() {
        let app = build_router(AppState::without_db());
        let resp = app
            .oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["status"], "ok");
    }

    #[tokio::test]
    async fn readyz_ok_without_db() {
        let app = build_router(AppState::without_db());
        let resp = app
            .oneshot(Request::builder().uri("/readyz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
```

- [ ] **Step 3: Run the tests to verify they pass**

Run: `cargo test -p egret-server`
Expected: PASS — `healthz_returns_200_ok`, `readyz_ok_without_db`.

- [ ] **Step 4: Commit**

```bash
git add crates/egret-server
git commit -m "feat(server): axum router with health and readiness endpoints"
```

---

### Task 6: `egret-bin` CLI entrypoint (serve / migrate / init)

**Files:**
- Create: `crates/egret-bin/Cargo.toml`
- Create: `crates/egret-bin/src/cli.rs`
- Create: `crates/egret-bin/src/main.rs`

- [ ] **Step 1: Create the crate manifest**

Create `crates/egret-bin/Cargo.toml`:

```toml
[package]
name = "egret-bin"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[[bin]]
name = "egret"
path = "src/main.rs"

[dependencies]
egret-core.workspace = true
egret-config.workspace = true
egret-metastore.workspace = true
egret-server.workspace = true
tokio.workspace = true
clap.workspace = true
anyhow.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
```

- [ ] **Step 2: Write the failing test for CLI parsing**

Create `crates/egret-bin/src/cli.rs`:

```rust
//! Command-line interface definition for the `egret` binary.

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "egret", version, about = "Egret data portal")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum Command {
    /// Run the HTTP server.
    Serve,
    /// Apply database migrations and exit.
    Migrate,
    /// Bootstrap the initial admin user and default workspace.
    Init,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_subcommand() {
        assert_eq!(Cli::parse_from(["egret", "serve"]).command, Command::Serve);
        assert_eq!(Cli::parse_from(["egret", "migrate"]).command, Command::Migrate);
        assert_eq!(Cli::parse_from(["egret", "init"]).command, Command::Init);
    }

    #[test]
    fn unknown_subcommand_is_rejected() {
        assert!(Cli::try_parse_from(["egret", "nope"]).is_err());
    }
}
```

- [ ] **Step 3: Write the binary entrypoint**

Create `crates/egret-bin/src/main.rs`:

```rust
//! `egret` binary entrypoint: parse the CLI, load config, dispatch the command.

mod cli;

use anyhow::Context;
use cli::{Cli, Command};
use clap::Parser;
use egret_config::Config;
use egret_metastore::Db;
use egret_server::{build_router, AppState};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config = Config::load().context("loading configuration")?;
    init_tracing(&config);

    match cli.command {
        Command::Serve => serve(config).await,
        Command::Migrate => migrate(config).await,
        Command::Init => {
            // Placeholder until Phase 1 (auth/workspace) lands.
            tracing::warn!("`init` is not implemented yet (arrives in Phase 1)");
            Ok(())
        }
    }
}

fn init_tracing(config: &Config) {
    let filter = EnvFilter::try_new(&config.log.filter)
        .unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    if config.log.json {
        builder.json().init();
    } else {
        builder.init();
    }
}

async fn migrate(config: Config) -> anyhow::Result<()> {
    let db = Db::connect(&config.database.url, config.database.max_connections)
        .await
        .context("connecting to database")?;
    db.migrate().await.context("running migrations")?;
    tracing::info!("migrations applied");
    Ok(())
}

async fn serve(config: Config) -> anyhow::Result<()> {
    let db = Db::connect(&config.database.url, config.database.max_connections)
        .await
        .context("connecting to database")?;
    db.migrate().await.context("running migrations on boot")?;

    let app = build_router(AppState { db: Some(db) });
    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    tracing::info!(%addr, "egret listening");
    axum::serve(listener, app).await.context("server error")?;
    Ok(())
}
```

- [ ] **Step 4: Run the CLI parsing tests to verify they pass**

Run: `cargo test -p egret-bin`
Expected: PASS — `parses_each_subcommand`, `unknown_subcommand_is_rejected`.

- [ ] **Step 5: Verify the whole workspace builds**

Run: `cargo build --workspace`
Expected: builds with no errors.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-bin
git commit -m "feat(bin): egret CLI with serve/migrate/init commands"
```

---

### Task 7: Local dev stack (Docker Compose) + env template + justfile

**Files:**
- Create: `.env.example`
- Create: `deploy/docker-compose.dev.yml`
- Create: `deploy/trino/catalog/.gitkeep`
- Create: `justfile`

- [ ] **Step 1: Create the env template**

Create `.env.example`:

```bash
# Postgres (matches docker-compose.dev.yml)
DATABASE_URL=postgres://egret:egret@localhost:5432/egret
EGRET_DATABASE__URL=postgres://egret:egret@localhost:5432/egret

# Server
EGRET_SERVER__HOST=0.0.0.0
EGRET_SERVER__PORT=8080

# Logging
EGRET_LOG__FILTER=info,egret=debug
EGRET_LOG__JSON=false

# MinIO (object storage — used from Phase 2)
AWS_ACCESS_KEY_ID=egret
AWS_SECRET_ACCESS_KEY=egretsecret
AWS_ENDPOINT_URL=http://localhost:9000
```

- [ ] **Step 2: Create the dev compose stack**

Create `deploy/docker-compose.dev.yml`:

```yaml
name: egret-dev

services:
  postgres:
    image: postgres:16
    environment:
      POSTGRES_USER: egret
      POSTGRES_PASSWORD: egret
      POSTGRES_DB: egret
    ports:
      - "5432:5432"
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U egret"]
      interval: 5s
      timeout: 3s
      retries: 10
    volumes:
      - pgdata:/var/lib/postgresql/data

  minio:
    image: minio/minio:latest
    command: server /data --console-address ":9001"
    environment:
      MINIO_ROOT_USER: egret
      MINIO_ROOT_PASSWORD: egretsecret
    ports:
      - "9000:9000"
      - "9001:9001"
    healthcheck:
      test: ["CMD", "mc", "ready", "local"]
      interval: 5s
      timeout: 3s
      retries: 10
    volumes:
      - miniodata:/data

  trino:
    image: trinodb/trino:latest
    ports:
      - "8090:8080"
    volumes:
      - ./trino/catalog:/etc/trino/catalog

  starrocks:
    image: starrocks/allin1-ubuntu:latest
    ports:
      - "9030:9030"   # MySQL protocol (query)
      - "8030:8030"   # FE HTTP
    healthcheck:
      test: ["CMD-SHELL", "mysql -h127.0.0.1 -P9030 -uroot -e 'select 1' || exit 1"]
      interval: 10s
      timeout: 5s
      retries: 20

volumes:
  pgdata:
  miniodata:
```

Create `deploy/trino/catalog/.gitkeep` (empty file — engine catalog config is added in Phase 2):

```text
```

- [ ] **Step 3: Create the justfile**

Create `justfile`:

```just
# Egret dev commands. Run `just --list` to see all.

set dotenv-load := true

# Start the local dev stack (Postgres, MinIO, Trino, StarRocks)
up:
    docker compose -f deploy/docker-compose.dev.yml up -d

# Stop the dev stack
down:
    docker compose -f deploy/docker-compose.dev.yml down

# Format, lint, and test everything
check:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace

# Apply migrations against the running Postgres
migrate:
    cargo run -p egret-bin -- migrate

# Run the server
serve:
    cargo run -p egret-bin -- serve
```

- [ ] **Step 4: Verify the compose file is valid**

Run: `docker compose -f deploy/docker-compose.dev.yml config -q`
Expected: no output, exit code 0 (the compose file parses).

- [ ] **Step 5: Smoke-test the DB-backed path (optional but recommended)**

Run:
```bash
docker compose -f deploy/docker-compose.dev.yml up -d postgres
# wait for healthy, then:
DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-metastore --test migrate
```
Expected: PASS — the integration test now connects and asserts `egret_meta` is seeded.

- [ ] **Step 6: Commit**

```bash
git add .env.example deploy justfile
git commit -m "chore(dev): docker-compose dev stack, env template, justfile"
```

---

### Task 8: Continuous integration (GitHub Actions)

**Files:**
- Create: `.github/workflows/ci.yml`

- [ ] **Step 1: Write the CI workflow**

Create `.github/workflows/ci.yml`:

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

env:
  CARGO_TERM_COLOR: always
  DATABASE_URL: postgres://egret:egret@localhost:5432/egret

jobs:
  rust:
    runs-on: ubuntu-latest
    services:
      postgres:
        image: postgres:16
        env:
          POSTGRES_USER: egret
          POSTGRES_PASSWORD: egret
          POSTGRES_DB: egret
        ports:
          - 5432:5432
        options: >-
          --health-cmd "pg_isready -U egret"
          --health-interval 5s
          --health-timeout 3s
          --health-retries 10
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - name: Format
        run: cargo fmt --all -- --check
      - name: Clippy
        run: cargo clippy --workspace --all-targets -- -D warnings
      - name: Test
        run: cargo test --workspace
```

- [ ] **Step 2: Verify formatting and clippy locally before relying on CI**

Run:
```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: both pass with no warnings. Fix any clippy findings inline (e.g. `len() >= 1` → clippy may prefer `!is_empty()`; if flagged, change `MIGRATOR.migrations.len() >= 1` to `!MIGRATOR.migrations.is_empty()` and re-run the metastore test).

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: fmt, clippy, and test on push and PR with postgres service"
```

---

## Self-Review

**Spec coverage (architecture §16 Phase 0 — backend portion):**
- "Cargo workspace + crate skeletons (egret-core, egret-config, egret-metastore, egret-server)" → Tasks 1–5. ✅ (`egret-bin` added as the entrypoint, Task 6.)
- "CI" → Task 8. ✅
- "migration runner" → Task 4 (`Db::migrate`, `MIGRATOR`) + Task 6 (`egret migrate`). ✅
- "dev Docker/compose (Postgres + MinIO + Trino + StarRocks)" → Task 7. ✅
- "FE skeleton: extract packages/ui from sparklabx, app shell + workspace switcher" → **deferred to a separate Phase 0b plan** (stated in the plan intro). ⚠️ intentional split.

**Placeholder scan:** No "TBD"/"add error handling"/"similar to Task N". The only deferred behavior is `Command::Init` (logs a warning, lands in Phase 1) and `deploy/trino/catalog` config (Phase 2) — both explicitly noted, not silent gaps.

**Type consistency:**
- `Db::connect(url, max_connections)` — signature matches its callers in Task 6 (`migrate`/`serve`) and Task 5's note. ✅
- `AppState { db: Option<Db> }` — constructed via `AppState::without_db()` (tests) and `AppState { db: Some(db) }` (Task 6). ✅
- `Config` fields (`server.host/port`, `database.url/max_connections`, `log.filter/json`) — used consistently in Tasks 3 and 6. ✅
- `MIGRATOR.migrations` field access — used in Task 4 test and `Db::migrate`. ✅
- `Pagination::offset/limit`, `WorkspaceContext::is_platform_admin` — defined and tested in Task 2, no external callers yet. ✅

No gaps requiring new tasks.

---

## Notes for the executor

- **Toolchain:** `rust-toolchain.toml` pins 1.82; if the local default differs, `rustup` auto-installs it on first `cargo` invocation.
- **Offline-green:** the full `cargo test --workspace` passes with no Postgres running (the only DB test self-skips). The DB path is exercised in CI (Postgres service) and locally via `just up` + Task 7 Step 5.
- **Engine images:** `trino`/`starrocks` in compose are for later phases; Phase 0a doesn't connect to them. They're included now so the dev stack is complete and stable for Phase 1+.
- **Next:** Phase 0b (frontend skeleton), then Phase 1 (auth/SSO + workspace + database registration) per the architecture roadmap.
