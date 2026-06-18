# Phase 3c-1 — SQL Lab Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A working SQL Lab in the Explorer SPA — pick a granted database, browse its schema, write SQL (Monaco + schema-aware autocomplete), run it async via the existing engine (REST polling), see paginated results, export CSV, save/load named queries, see history.

**Architecture:** New backend crate `egret-bi` (saved-query CRUD + per-engine schema introspection via the engine connectors). One new route on `egret-engine` for server-side CSV export. A React SQL Lab page consuming both. Backend follows the Phase 3a/3b-1 conventions; frontend follows the Phase 0b SPA patterns.

**Tech Stack:** Rust (axum 0.8, sqlx 0.8 runtime queries, async-trait); React 18 + TypeScript + Vite + `@tanstack/react-query` 5 + `react-resizable-panels` + `@monaco-editor/react` (new) + Vitest.

---

## Conventions

**Backend:** runtime sqlx only (no `query!`); DB-gated tests self-skip without `DATABASE_URL` (`postgres://egret:egret@localhost:5432/egret`); engine-gated tests self-skip without `EGRET_TEST_TRINO_URL`/`EGRET_TEST_STARROCKS_URL` (dev engines at :8090/:9030, both up); error→HTTP via `IntoResponse` (`DbApiError`/`EngineApiError` pattern); plain `git commit` (never `--author`); `cargo fmt --all` + `cargo clippy --workspace --all-targets -- -D warnings` clean before commit.

**Frontend:** `cd frontend`; `npm test` = `vitest run`; `npm run typecheck` = `tsc --noEmit`; `npm run lint`. API via `apiJson`/`apiFetch` from `@/api/client` (prefixes `/api`, sends `egret_csrf` cookie header on mutating requests, throws `UnauthorizedError`/`ApiError`). Hooks follow `@/api/hooks.ts` (`useQuery`/`useMutation`, queryKey arrays, `qc.invalidateQueries`). Path alias `@/` → `src/`. Tests: vitest + `@testing-library/react` + `QueryClientProvider` + `MemoryRouter`, fetch mocked.

## Reference facts (verified)

- `egret_engine::connector::{build_connector, ConnSpec, Connector}` are `pub`; `ConnSpec::from_database(&Database, Option<String>) -> ConnSpec`; `Connector::execute(sql, &Limits, CancellationToken) -> Result<QueryResult, EngineError>`. `egret_engine::model::{Limits, QueryResult, Column}`.
- `egret_database::DatabaseRepo::{get(id)->Result<Database,_>, open_secret(id)->Result<Option<String>,_>, list_for_workspace(ws)->Result<Vec<Database>,_>}`; `egret_database::model::{Engine, ConnOptions, Database}` (`Database{id,name,engine,host,port,options,has_secret,status}`).
- `egret_engine` routes (`crates/egret-engine/src/routes.rs`): `engine_router()` mounts `/ws/{workspace}/queries…`; `EngineState{store: QueryStore, …, storage: Arc<dyn ObjectStore>, …, sessions, workspaces, authz}`; helper `async fn owned(&EngineState, &WorkspaceContext, Uuid) -> Result<Query, EngineApiError>`; `QueryStore::read_results(&Arc<dyn ObjectStore>, uri:&str, offset, limit) -> Result<Vec<serde_json::Value>, EngineError>`; `Query{columns: Option<Value>, result_ref: Option<String>, row_count: Option<i64>, …}`.
- RBAC: `egret_workspace::context::WsContext` (needs `SessionService`+`WorkspaceRepo`+`Authorizer` `FromRef`), `Authorizer::require_workspace(&ctx,"query:run")`. `WorkspaceContext{workspace_id:WorkspaceId(.0), principal:Principal{user_id:UserId(.0), platform_roles}}`.
- `egret_metastore::Db{pool}`; migrator embeds `../../migrations` (0010 auto-applies after a metastore recompile).
- Frontend routing: `/explore/:workspace/*` → `sql-lab` → `SqlLab` (`crates`… no — `frontend/src/App.tsx:46`). The workspace slug is the `:workspace` route param (`useParams`).

## File structure

```text
Cargo.toml                                  # MODIFY: workspace dep egret-bi
migrations/0010_sqllab.sql                  # CREATE
crates/egret-bi/                            # CREATE crate (lib, model, error, saved, schema, routes)
crates/egret-engine/src/routes.rs           # MODIFY: results.csv route
crates/egret-server/src/lib.rs              # MODIFY: AppState.bi + mount
crates/egret-bin/src/main.rs                # MODIFY: build BiState
frontend/package.json                       # MODIFY: add @monaco-editor/react
frontend/src/api/sqllab.ts                  # CREATE: types + hooks
frontend/src/routes/explore/SqlLab.tsx      # REPLACE placeholder
frontend/src/components/sqllab/             # CREATE: SchemaTree, ResultsGrid, SavedQueries, History, SqlEditor
frontend/src/components/sqllab/__tests__/   # CREATE: tests
```

---

## Task 1: Scaffold `egret-bi` crate + frontend Monaco dep

**Files:** Modify `Cargo.toml`; Create `crates/egret-bi/{Cargo.toml,src/lib.rs,...}`; Modify `frontend/package.json`.

- [ ] **Step 1: Workspace dep** — root `Cargo.toml` `[workspace.dependencies]` add:
```toml
egret-bi = { path = "crates/egret-bi" }
```

- [ ] **Step 2: `crates/egret-bi/Cargo.toml`**
```toml
[package]
name = "egret-bi"
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
egret-engine.workspace = true
axum.workspace = true
sqlx.workspace = true
serde.workspace = true
serde_json.workspace = true
uuid.workspace = true
chrono.workspace = true
thiserror.workspace = true
tracing.workspace = true
tokio.workspace = true
tokio-util.workspace = true
async-trait.workspace = true

[dev-dependencies]
tower.workspace = true
http-body-util = "0.1"
egret-secrets.workspace = true
url = { workspace = true }
```

- [ ] **Step 3: `crates/egret-bi/src/lib.rs` (declare modules; placeholders filled by later tasks)**
```rust
//! BI layer: SQL Lab (saved queries + schema introspection). Dataset/chart/dashboard come later.

mod error;
pub mod model;
mod routes;
mod saved;
mod schema;
```
Create `error.rs`, `routes.rs`, `saved.rs`, `schema.rs`, and `model.rs` as `//! placeholder` files (model is `pub mod`). No `pub use` yet — later tasks add them. `cargo build --workspace` must be GREEN.

- [ ] **Step 4: Frontend Monaco dep** — in `frontend/`, run:
```bash
cd frontend && npm install @monaco-editor/react@^4.6.0
```
This adds `@monaco-editor/react` (and `monaco-editor` peer) to `package.json`. (The component lazy-loads Monaco from a CDN by default; that's fine for dev. If offline builds are required later, switch to the `loader` config — out of scope here.)

- [ ] **Step 5: Verify** `cargo build --workspace` PASS; `cd frontend && npm install` clean.
- [ ] **Step 6: Commit**
```bash
git add Cargo.toml crates/egret-bi frontend/package.json frontend/package-lock.json
git commit -m "feat(bi): scaffold egret-bi crate + add Monaco dep"
```

---

## Task 2: `egret-bi` model + error

**Files:** Create `crates/egret-bi/src/model.rs`, `crates/egret-bi/src/error.rs`; update `lib.rs`.

- [ ] **Step 1: failing unit test** (append to `model.rs`)
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schema_node_serializes() {
        let c = SchemaColumn { name: "id".into(), ty: "bigint".into() };
        assert_eq!(serde_json::to_value(&c).unwrap(), serde_json::json!({"name":"id","type":"bigint"}));
    }
}
```

- [ ] **Step 2: Run** `cargo test -p egret-bi model` → compile FAIL.

- [ ] **Step 3: `crates/egret-bi/src/model.rs`**
```rust
//! SQL Lab DTOs.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct SaveQuery {
    pub database_id: Option<Uuid>,
    pub name: String,
    pub sql: String,
}

#[derive(Debug, Serialize)]
pub struct SavedQueryDto {
    pub id: String,
    pub database_id: Option<String>,
    pub name: String,
    pub sql: String,
    pub created_by: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// A single column in a DESCRIBE result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaColumn {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}
```

- [ ] **Step 4: `crates/egret-bi/src/error.rs`**
```rust
//! BI error + HTTP mapping.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

#[derive(Debug, thiserror::Error)]
pub enum BiError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("forbidden")]
    Forbidden,
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("engine: {0}")]
    Engine(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

impl From<egret_database::DatabaseError> for BiError {
    fn from(e: egret_database::DatabaseError) -> Self {
        match e {
            egret_database::DatabaseError::NotFound(s) => BiError::NotFound(s),
            other => BiError::Engine(other.to_string()),
        }
    }
}
impl From<egret_engine::EngineError> for BiError {
    fn from(e: egret_engine::EngineError) -> Self {
        match e {
            egret_engine::EngineError::NotFound(s) => BiError::NotFound(s),
            other => BiError::Engine(other.to_string()),
        }
    }
}

pub enum BiApiError {
    Auth(egret_core::Error),
    Domain(BiError),
}
impl From<egret_core::Error> for BiApiError {
    fn from(e: egret_core::Error) -> Self { BiApiError::Auth(e) }
}
impl From<BiError> for BiApiError {
    fn from(e: BiError) -> Self { BiApiError::Domain(e) }
}

impl IntoResponse for BiApiError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            BiApiError::Auth(e) => match e {
                egret_core::Error::NotFound(_) => (StatusCode::NOT_FOUND, "not found".to_string()),
                egret_core::Error::InvalidInput(m) => (StatusCode::BAD_REQUEST, m),
                egret_core::Error::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".to_string()),
                egret_core::Error::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
                egret_core::Error::Conflict(m) => (StatusCode::CONFLICT, m),
                egret_core::Error::TooManyRequests => (StatusCode::TOO_MANY_REQUESTS, "too many requests".to_string()),
                egret_core::Error::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal error".to_string()),
            },
            BiApiError::Domain(e) => match &e {
                BiError::NotFound(_) => (StatusCode::NOT_FOUND, e.to_string()),
                BiError::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
                BiError::BadRequest(_) => (StatusCode::BAD_REQUEST, e.to_string()),
                BiError::Engine(_) => (StatusCode::BAD_GATEWAY, e.to_string()),
                BiError::Db(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal error".to_string()),
            },
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}
```

- [ ] **Step 5: `lib.rs`** — add `pub use error::{BiApiError, BiError};`.
- [ ] **Step 6: Run** `cargo test -p egret-bi model` → PASS; `cargo build -p egret-bi` clean (no warnings — `model` is `pub`, errors are `pub use`-exported).
- [ ] **Step 7: Commit**
```bash
cargo fmt --all && git add crates/egret-bi/src && git commit -m "feat(bi): SQL Lab model + error types"
```

---

## Task 3: Migration `0010` + `SavedQueryRepo` (DB-gated)

**Files:** Create `migrations/0010_sqllab.sql`, `crates/egret-bi/src/saved.rs`, `crates/egret-bi/tests/saved.rs`; update `lib.rs`.

- [ ] **Step 1: `migrations/0010_sqllab.sql`**
```sql
CREATE TABLE saved_queries (
  id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  database_id  uuid REFERENCES databases(id) ON DELETE SET NULL,
  name         text NOT NULL,
  sql          text NOT NULL,
  created_by   uuid REFERENCES users(id),
  created_at   timestamptz NOT NULL DEFAULT now(),
  updated_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX saved_queries_ws_idx ON saved_queries (workspace_id, updated_at DESC);
```

- [ ] **Step 2: failing DB-gated tests** `crates/egret-bi/tests/saved.rs`
```rust
//! DB-gated SavedQueryRepo tests. Self-skip without DATABASE_URL.
use egret_bi::SavedQueryRepo;
use egret_metastore::Db;
use uuid::Uuid;

async fn setup() -> Option<(SavedQueryRepo, Uuid, Uuid)> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let uid: Uuid = sqlx::query_scalar("INSERT INTO users (email, provider) VALUES ($1,'local') RETURNING id")
        .bind(format!("u_{}@t.local", Uuid::new_v4().simple())).fetch_one(&db.pool).await.unwrap();
    let wid: Uuid = sqlx::query_scalar("INSERT INTO workspaces (slug,name,created_by) VALUES ($1,'t',$2) RETURNING id")
        .bind(format!("w-{}", Uuid::new_v4().simple())).bind(uid).fetch_one(&db.pool).await.unwrap();
    Some((SavedQueryRepo::new(db), wid, uid))
}

#[tokio::test]
async fn crud_roundtrip() {
    let Some((r, wid, uid)) = setup().await else { eprintln!("skip"); return; };
    let q = r.create(wid, None, "q1", "SELECT 1", Some(egret_core::UserId(uid))).await.unwrap();
    assert_eq!(q.name, "q1");
    assert!(r.list(wid).await.unwrap().iter().any(|s| s.id == q.id));
    let got = r.get(q.id).await.unwrap().unwrap();
    assert_eq!(got.sql, "SELECT 1");
    let upd = r.update(q.id, "q1b", "SELECT 2", None).await.unwrap();
    assert_eq!(upd.name, "q1b");
    assert_eq!(upd.sql, "SELECT 2");
    r.delete(q.id).await.unwrap();
    assert!(r.get(q.id).await.unwrap().is_none());
}
```

- [ ] **Step 3: Run** `DATABASE_URL=… cargo test -p egret-bi --test saved` → compile FAIL.

- [ ] **Step 4: `crates/egret-bi/src/saved.rs`**
```rust
//! Saved-query repository.

use egret_core::UserId;
use egret_metastore::Db;
use uuid::Uuid;

use crate::error::BiError;

#[derive(Clone)]
pub struct SavedQueryRepo {
    db: Db,
}

#[derive(sqlx::FromRow)]
pub struct SavedQuery {
    pub id: Uuid,
    pub database_id: Option<Uuid>,
    pub name: String,
    pub sql: String,
    pub created_by: Option<Uuid>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

const COLS: &str = "id, database_id, name, sql, created_by, updated_at";

impl SavedQueryRepo {
    pub fn new(db: Db) -> Self { Self { db } }

    pub async fn create(&self, ws: Uuid, db_id: Option<Uuid>, name: &str, sql: &str, user: Option<UserId>) -> Result<SavedQuery, BiError> {
        let row = sqlx::query_as::<_, SavedQuery>(&format!(
            "INSERT INTO saved_queries (workspace_id, database_id, name, sql, created_by) \
             VALUES ($1,$2,$3,$4,$5) RETURNING {COLS}"))
            .bind(ws).bind(db_id).bind(name).bind(sql).bind(user.map(|u| u.0))
            .fetch_one(&self.db.pool).await?;
        Ok(row)
    }

    pub async fn list(&self, ws: Uuid) -> Result<Vec<SavedQuery>, BiError> {
        let rows = sqlx::query_as::<_, SavedQuery>(&format!(
            "SELECT {COLS} FROM saved_queries WHERE workspace_id=$1 ORDER BY updated_at DESC"))
            .bind(ws).fetch_all(&self.db.pool).await?;
        Ok(rows)
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<SavedQuery>, BiError> {
        let row = sqlx::query_as::<_, SavedQuery>(&format!("SELECT {COLS} FROM saved_queries WHERE id=$1"))
            .bind(id).fetch_optional(&self.db.pool).await?;
        Ok(row)
    }

    pub async fn update(&self, id: Uuid, name: &str, sql: &str, db_id: Option<Uuid>) -> Result<SavedQuery, BiError> {
        let row = sqlx::query_as::<_, SavedQuery>(&format!(
            "UPDATE saved_queries SET name=$2, sql=$3, database_id=$4, updated_at=now() WHERE id=$1 RETURNING {COLS}"))
            .bind(id).bind(name).bind(sql).bind(db_id)
            .fetch_optional(&self.db.pool).await?
            .ok_or_else(|| BiError::NotFound(id.to_string()))?;
        Ok(row)
    }

    pub async fn delete(&self, id: Uuid) -> Result<(), BiError> {
        sqlx::query("DELETE FROM saved_queries WHERE id=$1").bind(id).execute(&self.db.pool).await?;
        Ok(())
    }

    /// The owner's id (for the owner-or-admin edit/delete gate).
    pub async fn owner(&self, id: Uuid) -> Result<Option<Uuid>, BiError> {
        Ok(sqlx::query_scalar("SELECT created_by FROM saved_queries WHERE id=$1")
            .bind(id).fetch_optional(&self.db.pool).await?.flatten())
    }
}
```

- [ ] **Step 5: `lib.rs`** — add `pub use saved::{SavedQuery, SavedQueryRepo};`.
- [ ] **Step 6: Run** `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-bi --test saved` → 1 passed. `cargo build -p egret-bi` clean.
- [ ] **Step 7: Commit**
```bash
cargo fmt --all && git add migrations/0010_sqllab.sql crates/egret-bi/src crates/egret-bi/tests/saved.rs && git commit -m "feat(bi): 0010 migration + SavedQueryRepo"
```

---

## Task 4: `SchemaService` per-engine introspection (engine-gated)

**Files:** Create `crates/egret-bi/src/schema.rs`, `crates/egret-bi/tests/schema.rs`; update `lib.rs`.

- [ ] **Step 1: failing engine-gated test** `crates/egret-bi/tests/schema.rs`
```rust
//! Engine-gated SchemaService test. Self-skip unless EGRET_TEST_TRINO_URL is set.
use egret_bi::SchemaService;
use egret_database::model::{ConnOptions, Engine};

fn trino_spec() -> Option<egret_engine::connector::ConnSpec> {
    let base = std::env::var("EGRET_TEST_TRINO_URL").ok()?;
    let u = url::Url::parse(&base).ok()?;
    Some(egret_engine::connector::ConnSpec {
        engine: Engine::Trino,
        host: u.host_str()?.to_string(),
        port: u.port().unwrap_or(8080) as i32,
        options: ConnOptions { user: Some("admin".into()), http_scheme: Some(u.scheme().to_string()), ..Default::default() },
        secret: None,
    })
}

#[tokio::test]
async fn trino_lists_catalogs_and_schemas() {
    let Some(spec) = trino_spec() else { eprintln!("skip"); return; };
    let svc = SchemaService::new();
    let cats = svc.catalogs(&spec).await.unwrap();
    assert!(cats.iter().any(|c| c == "system" || c == "tpch" || !cats.is_empty()));
    // every Trino has the `system` catalog with an `information_schema` or `runtime` schema
    if cats.iter().any(|c| c == "system") {
        let schemas = svc.schemas(&spec, "system").await.unwrap();
        assert!(!schemas.is_empty());
    }
}
```
> **Implementer note:** `system` catalog exists on every Trino. If the assert is flaky on this image, assert simply `!cats.is_empty()` and that `schemas(&spec, &cats[0])` returns Ok. Verify against the live Trino at :8090 and adjust the assertions to something stable, keeping the test meaningful.

- [ ] **Step 2: Run** `EGRET_TEST_TRINO_URL=http://localhost:8090 cargo test -p egret-bi --test schema` → compile FAIL.

- [ ] **Step 3: `crates/egret-bi/src/schema.rs`**
```rust
//! Per-engine schema introspection via the engine connectors (synchronous, small queries).

use std::time::Duration;

use egret_database::model::Engine;
use egret_engine::connector::{build_connector, ConnSpec};
use egret_engine::model::Limits;
use tokio_util::sync::CancellationToken;

use crate::error::BiError;
use crate::model::SchemaColumn;

#[derive(Clone, Default)]
pub struct SchemaService;

impl SchemaService {
    pub fn new() -> Self { SchemaService }

    fn limits() -> Limits {
        Limits { max_rows: 10_000, timeout: Duration::from_secs(15) }
    }

    /// Run a meta-query and return the first column of every row as a String.
    async fn first_col(&self, spec: &ConnSpec, sql: &str) -> Result<Vec<String>, BiError> {
        let conn = build_connector(spec);
        let r = conn.execute(sql, &Self::limits(), CancellationToken::new()).await?;
        Ok(r.rows.into_iter().filter_map(|row| row.into_iter().next()).map(val_to_string).collect())
    }

    pub async fn catalogs(&self, spec: &ConnSpec) -> Result<Vec<String>, BiError> {
        match spec.engine {
            Engine::Trino => self.first_col(spec, "SHOW CATALOGS").await,
            Engine::StarRocks => Ok(vec![]), // no catalog level
        }
    }

    pub async fn schemas(&self, spec: &ConnSpec, catalog: &str) -> Result<Vec<String>, BiError> {
        match spec.engine {
            Engine::Trino => self.first_col(spec, &format!("SHOW SCHEMAS FROM {}", quote_trino(catalog))).await,
            Engine::StarRocks => self.first_col(spec, "SHOW DATABASES").await,
        }
    }

    pub async fn tables(&self, spec: &ConnSpec, catalog: &str, schema: &str) -> Result<Vec<String>, BiError> {
        match spec.engine {
            Engine::Trino => self.first_col(spec, &format!("SHOW TABLES FROM {}.{}", quote_trino(catalog), quote_trino(schema))).await,
            Engine::StarRocks => self.first_col(spec, &format!("SHOW TABLES FROM {}", quote_mysql(schema))).await,
        }
    }

    pub async fn columns(&self, spec: &ConnSpec, catalog: &str, schema: &str, table: &str) -> Result<Vec<SchemaColumn>, BiError> {
        let conn = build_connector(spec);
        let sql = match spec.engine {
            Engine::Trino => format!("DESCRIBE {}.{}.{}", quote_trino(catalog), quote_trino(schema), quote_trino(table)),
            Engine::StarRocks => format!("DESCRIBE {}.{}", quote_mysql(schema), quote_mysql(table)),
        };
        let r = conn.execute(&sql, &Self::limits(), CancellationToken::new()).await?;
        // both engines: column name in col 0, type in col 1
        Ok(r.rows.into_iter().map(|row| {
            let mut it = row.into_iter();
            SchemaColumn {
                name: it.next().map(val_to_string).unwrap_or_default(),
                ty: it.next().map(val_to_string).unwrap_or_default(),
            }
        }).collect())
    }
}

fn val_to_string(v: serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s,
        other => other.to_string(),
    }
}

/// Quote a Trino identifier: wrap in double-quotes, escape internal quotes.
fn quote_trino(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}
/// Quote a MySQL/StarRocks identifier: backticks, escape internal backticks.
fn quote_mysql(ident: &str) -> String {
    format!("`{}`", ident.replace('`', "``"))
}
```

- [ ] **Step 4: `lib.rs`** — add `pub use schema::SchemaService;`.
- [ ] **Step 5: Run** `EGRET_TEST_TRINO_URL=http://localhost:8090 cargo test -p egret-bi --test schema` → passes against live Trino. Without the env var → skip. `cargo build -p egret-bi` + `cargo clippy -p egret-bi --all-targets -- -D warnings` clean.
- [ ] **Step 6: Commit**
```bash
cargo fmt --all && git add crates/egret-bi/src crates/egret-bi/tests/schema.rs && git commit -m "feat(bi): per-engine schema introspection"
```

---

## Task 5: `egret-bi` routes + RBAC + wiring + HTTP tests

**Files:** Create `crates/egret-bi/src/routes.rs`, `crates/egret-bi/tests/http.rs`; Modify `crates/egret-server/{Cargo.toml,src/lib.rs}`, `crates/egret-bin/{Cargo.toml,src/main.rs}`; update `lib.rs`.

- [ ] **Step 1: failing HTTP test** `crates/egret-bi/tests/http.rs`
```rust
//! Router smoke test — unauthenticated requests rejected.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_bi::{sqllab_router, BiState};
use tower::ServiceExt;

async fn state() -> Option<BiState> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = egret_metastore::Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(BiState::for_test(db))
}

#[tokio::test]
async fn saved_list_requires_auth() {
    let Some(st) = state().await else { eprintln!("skip"); return; };
    let app = sqllab_router().with_state(st);
    let resp = app.oneshot(Request::builder().uri("/ws/eng/sqllab/queries").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
```
> **Implementer note:** like the engine's test ctor, add `BiState::for_test(db)` building the sub-states (`SavedQueryRepo`, `SchemaService`, `DatabaseRepo` with a no-key `SecretStore`, `SessionService`, `WorkspaceRepo`, `Authorizer`). Keep it a `pub fn` (egret-secrets is a dev-dep only — so put `for_test` behind `#[cfg(test)]`? No: integration tests need it. To avoid shipping it in the lib, follow the 3b-1 final-review lesson and build the state INLINE in `tests/http.rs` instead of a lib `for_test`. Use that approach: a local `build_state(db)` helper in the test file; do NOT add `for_test` to the library, and keep `egret-secrets` a dev-dependency.)

Adjust the test to call a local `build_state(db)` helper (see the note). The helper mirrors the engine's test pattern.

- [ ] **Step 2: Run** `DATABASE_URL=… cargo test -p egret-bi --test http` → compile FAIL.

- [ ] **Step 3: `crates/egret-bi/src/routes.rs`**
```rust
//! SQL Lab routes: saved queries + schema browse.

use axum::extract::{FromRef, Path, Query as AxQuery, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use egret_auth::session::SessionService;
use egret_core::Error;
use egret_database::DatabaseRepo;
use egret_engine::connector::ConnSpec;
use egret_workspace::context::WsContext;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use serde::Deserialize;
use uuid::Uuid;

use crate::error::BiApiError;
use crate::model::{SaveQuery, SavedQueryDto};
use crate::saved::SavedQueryRepo;
use crate::schema::SchemaService;

#[derive(Clone)]
pub struct BiState {
    pub saved: SavedQueryRepo,
    pub schema: SchemaService,
    pub databases: DatabaseRepo,
    pub sessions: SessionService,
    pub workspaces: WorkspaceRepo,
    pub authz: Authorizer,
}
impl FromRef<BiState> for SessionService { fn from_ref(s: &BiState) -> Self { s.sessions.clone() } }
impl FromRef<BiState> for WorkspaceRepo { fn from_ref(s: &BiState) -> Self { s.workspaces.clone() } }
impl FromRef<BiState> for Authorizer { fn from_ref(s: &BiState) -> Self { s.authz.clone() } }

const PERM: &str = "query:run";

pub fn sqllab_router() -> Router<BiState> {
    Router::new()
        .route("/ws/{workspace}/sqllab/queries", post(save).get(list))
        .route("/ws/{workspace}/sqllab/queries/{id}", get(load).put(update).delete(remove))
        .route("/ws/{workspace}/sqllab/databases/{db}/catalogs", get(catalogs))
        .route("/ws/{workspace}/sqllab/databases/{db}/schemas", get(schemas))
        .route("/ws/{workspace}/sqllab/databases/{db}/tables", get(tables))
        .route("/ws/{workspace}/sqllab/databases/{db}/columns", get(columns))
}

fn dto(q: crate::saved::SavedQuery) -> SavedQueryDto {
    SavedQueryDto {
        id: q.id.to_string(),
        database_id: q.database_id.map(|d| d.to_string()),
        name: q.name, sql: q.sql,
        created_by: q.created_by.map(|u| u.to_string()),
        updated_at: q.updated_at,
    }
}

fn is_admin(ctx: &egret_core::WorkspaceContext) -> bool {
    ctx.principal.platform_roles.iter().any(|r| r == "platform_admin")
}

// ── saved queries ──
async fn save(State(st): State<BiState>, WsContext(ctx): WsContext, Json(b): Json<SaveQuery>) -> Result<Json<SavedQueryDto>, BiApiError> {
    st.authz.require_workspace(&ctx, PERM).await.map_err(BiApiError::from)?;
    let q = st.saved.create(ctx.workspace_id.0, b.database_id, &b.name, &b.sql, Some(ctx.principal.user_id)).await.map_err(BiApiError::from)?;
    Ok(Json(dto(q)))
}
async fn list(State(st): State<BiState>, WsContext(ctx): WsContext) -> Result<Json<Vec<SavedQueryDto>>, BiApiError> {
    st.authz.require_workspace(&ctx, PERM).await.map_err(BiApiError::from)?;
    let qs = st.saved.list(ctx.workspace_id.0).await.map_err(BiApiError::from)?;
    Ok(Json(qs.into_iter().map(dto).collect()))
}
async fn load(State(st): State<BiState>, WsContext(ctx): WsContext, Path((_w, id)): Path<(String, Uuid)>) -> Result<Json<SavedQueryDto>, BiApiError> {
    st.authz.require_workspace(&ctx, PERM).await.map_err(BiApiError::from)?;
    let q = st.saved.get(id).await.map_err(BiApiError::from)?.ok_or(Error::NotFound("saved query".into()))?;
    Ok(Json(dto(q)))
}
async fn update(State(st): State<BiState>, WsContext(ctx): WsContext, Path((_w, id)): Path<(String, Uuid)>, Json(b): Json<SaveQuery>) -> Result<Json<SavedQueryDto>, BiApiError> {
    st.authz.require_workspace(&ctx, PERM).await.map_err(BiApiError::from)?;
    require_owner(&st, &ctx, id).await?;
    let q = st.saved.update(id, &b.name, &b.sql, b.database_id).await.map_err(BiApiError::from)?;
    Ok(Json(dto(q)))
}
async fn remove(State(st): State<BiState>, WsContext(ctx): WsContext, Path((_w, id)): Path<(String, Uuid)>) -> Result<Json<serde_json::Value>, BiApiError> {
    st.authz.require_workspace(&ctx, PERM).await.map_err(BiApiError::from)?;
    require_owner(&st, &ctx, id).await?;
    st.saved.delete(id).await.map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}
async fn require_owner(st: &BiState, ctx: &egret_core::WorkspaceContext, id: Uuid) -> Result<(), BiApiError> {
    let owner = st.saved.owner(id).await.map_err(BiApiError::from)?;
    if is_admin(ctx) || owner == Some(ctx.principal.user_id.0) { Ok(()) } else { Err(Error::Forbidden.into()) }
}

// ── schema browse ──
async fn conn_spec(st: &BiState, ctx: &egret_core::WorkspaceContext, db: Uuid) -> Result<ConnSpec, BiApiError> {
    st.authz.require_workspace(ctx, PERM).await.map_err(BiApiError::from)?;
    let granted = st.databases.list_for_workspace(ctx.workspace_id.0).await.map_err(|e| BiApiError::from(crate::BiError::from(e)))?;
    if !granted.iter().any(|d| d.id == db) {
        return Err(Error::NotFound("database not granted".into()).into());
    }
    let database = st.databases.get(db).await.map_err(|e| BiApiError::from(crate::BiError::from(e)))?;
    let secret = st.databases.open_secret(db).await.map_err(|e| BiApiError::from(crate::BiError::from(e)))?;
    Ok(ConnSpec::from_database(&database, secret))
}

#[derive(Deserialize)]
struct CatQ { catalog: Option<String> }
#[derive(Deserialize)]
struct SchemaQ { catalog: Option<String>, schema: Option<String> }
#[derive(Deserialize)]
struct ColQ { catalog: Option<String>, schema: Option<String>, table: Option<String> }

async fn catalogs(State(st): State<BiState>, WsContext(ctx): WsContext, Path((_w, db)): Path<(String, Uuid)>) -> Result<Json<Vec<String>>, BiApiError> {
    let spec = conn_spec(&st, &ctx, db).await?;
    Ok(Json(st.schema.catalogs(&spec).await.map_err(BiApiError::from)?))
}
async fn schemas(State(st): State<BiState>, WsContext(ctx): WsContext, Path((_w, db)): Path<(String, Uuid)>, AxQuery(q): AxQuery<CatQ>) -> Result<Json<Vec<String>>, BiApiError> {
    let spec = conn_spec(&st, &ctx, db).await?;
    Ok(Json(st.schema.schemas(&spec, q.catalog.as_deref().unwrap_or("")).await.map_err(BiApiError::from)?))
}
async fn tables(State(st): State<BiState>, WsContext(ctx): WsContext, Path((_w, db)): Path<(String, Uuid)>, AxQuery(q): AxQuery<SchemaQ>) -> Result<Json<Vec<String>>, BiApiError> {
    let spec = conn_spec(&st, &ctx, db).await?;
    Ok(Json(st.schema.tables(&spec, q.catalog.as_deref().unwrap_or(""), q.schema.as_deref().unwrap_or("")).await.map_err(BiApiError::from)?))
}
async fn columns(State(st): State<BiState>, WsContext(ctx): WsContext, Path((_w, db)): Path<(String, Uuid)>, AxQuery(q): AxQuery<ColQ>) -> Result<Json<Vec<crate::model::SchemaColumn>>, BiApiError> {
    let spec = conn_spec(&st, &ctx, db).await?;
    Ok(Json(st.schema.columns(&spec, q.catalog.as_deref().unwrap_or(""), q.schema.as_deref().unwrap_or(""), q.table.as_deref().unwrap_or("")).await.map_err(BiApiError::from)?))
}
```

- [ ] **Step 4: `lib.rs`** — add `pub use routes::{sqllab_router, BiState};`.

- [ ] **Step 5: Wire** — `egret-server/Cargo.toml` + `egret-bin/Cargo.toml` add `egret-bi.workspace = true`. In `crates/egret-server/src/lib.rs`: add `pub bi: Option<egret_bi::BiState>` to `AppState` (+ `bi: None` in `without_db()`), and after the engine mount block:
```rust
    if let Some(bi) = state.bi.clone() {
        let group = Router::new()
            .nest("/api", egret_bi::sqllab_router())
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(bi);
        router = router.merge(group);
    }
```
In `crates/egret-bin/src/main.rs` `serve()`, after the engine block, build `bi` (reuse the already-built `databases: DatabaseRepo`, `sessions`, and `WorkspaceRepo`/`Authorizer::new(db.clone())`):
```rust
    let bi = Some(egret_bi::BiState {
        saved: egret_bi::SavedQueryRepo::new(db.clone()),
        schema: egret_bi::SchemaService::new(),
        databases: databases.clone(),
        sessions: sessions.clone(),
        workspaces: WorkspaceRepo::new(db.clone()),
        authz: Authorizer::new(db.clone()),
    });
```
and add `bi,` to the `AppState { .. }` literal.
> **Implementer note:** `databases` is the `DatabaseRepo` built in the engine/database block — make sure it's still in scope (it is `Clone`). If it was moved into `EngineState`, clone it before, or rebuild `DatabaseRepo::new(db.clone(), egret_secrets::SecretStore::new(db.clone(), crypto.clone()))` — keep DRY.

- [ ] **Step 6: Verify** `cargo build --workspace`; `cargo clippy --workspace --all-targets -- -D warnings` clean; `DATABASE_URL=… cargo test -p egret-bi` → all pass; `cargo test -p egret-server` health green.
- [ ] **Step 7: Commit**
```bash
cargo fmt --all && git add crates/egret-bi crates/egret-server crates/egret-bin && git commit -m "feat(bi): SQL Lab routes + RBAC + wiring"
```

---

## Task 6: `egret-engine` `results.csv` route (Decision ①)

**Files:** Modify `crates/egret-engine/src/routes.rs`; Create `crates/egret-engine/tests/results_csv.rs`.

- [ ] **Step 1: failing DB-gated test** `crates/egret-engine/tests/results_csv.rs`
```rust
//! DB-gated CSV unit test for the JSONL→CSV conversion helper.
use egret_engine::routes::rows_to_csv;

#[test]
fn csv_escapes_and_headers() {
    let columns = serde_json::json!([{"name":"n","type":"bigint"},{"name":"s","type":"varchar"}]);
    let rows = vec![serde_json::json!([1, "a,b"]), serde_json::json!([2, "x\"y"])];
    let csv = rows_to_csv(&columns, &rows);
    assert_eq!(csv, "n,s\n1,\"a,b\"\n2,\"x\"\"y\"\n");
}
```
> **Implementer note:** expose `pub fn rows_to_csv(columns: &serde_json::Value, rows: &[serde_json::Value]) -> String` from `routes.rs` (or a small `csv` module) so it's unit-testable without HTTP. The HTTP route is covered by the e2e/manual flow; this test pins the RFC-4180 escaping.

- [ ] **Step 2: Run** `cargo test -p egret-engine --test results_csv` → compile FAIL.

- [ ] **Step 3: Add the route + helper to `crates/egret-engine/src/routes.rs`**

Add to `engine_router()`:
```rust
        .route("/ws/{workspace}/queries/{id}/results.csv", get(results_csv))
```
Add the handler + helper (and make `routes` reachable — it's already `pub use routes::…` in lib; add `pub fn rows_to_csv`):
```rust
async fn results_csv(
    State(st): State<EngineState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, uuid::Uuid)>,
) -> Result<axum::response::Response, EngineApiError> {
    st.authz.require_workspace(&ctx, PERM).await.map_err(EngineApiError::from)?;
    let q = owned(&st, &ctx, id).await?;
    let rows = match &q.result_ref {
        Some(uri) => {
            let n = q.row_count.unwrap_or(0).max(0) as usize;
            st.store.read_results(&st.storage, uri, 0, n).await.map_err(EngineApiError::from)?
        }
        None => vec![],
    };
    let columns = q.columns.clone().unwrap_or(serde_json::json!([]));
    let body = rows_to_csv(&columns, &rows);
    use axum::http::header;
    Ok((
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"query-{id}.csv\"")),
        ],
        body,
    ).into_response())
}

/// Build a CSV (RFC 4180) from a columns spec `[{name,type}]` + JSONL rows (`[[v,...]]`).
pub fn rows_to_csv(columns: &serde_json::Value, rows: &[serde_json::Value]) -> String {
    let mut out = String::new();
    if let Some(cols) = columns.as_array() {
        let headers: Vec<String> = cols.iter()
            .map(|c| c.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string())
            .collect();
        out.push_str(&headers.iter().map(|h| csv_field(h)).collect::<Vec<_>>().join(","));
        out.push('\n');
    }
    for row in rows {
        if let Some(arr) = row.as_array() {
            let line = arr.iter().map(|v| csv_field(&cell(v))).collect::<Vec<_>>().join(",");
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

fn cell(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}
```
> **Implementer note:** `routes` is a private module re-exported via `pub use`. To make `rows_to_csv` reachable from the integration test (`egret_engine::routes::rows_to_csv`), either make the module `pub mod routes;` in `lib.rs` (it currently is `mod routes;` with `pub use routes::{engine_router, EngineState}`), OR re-export the fn: add `pub use routes::rows_to_csv;` to `lib.rs` and change the test import to `use egret_engine::rows_to_csv;`. Pick the re-export (keeps the module private). Confirm `owned`, `PERM`, `EngineApiError`, `read_results` are in scope in `routes.rs` (they are).

- [ ] **Step 4: Run** `cargo test -p egret-engine --test results_csv` → PASS; `cargo build -p egret-engine` + clippy clean.
- [ ] **Step 5: Commit**
```bash
cargo fmt --all && git add crates/egret-engine/src/routes.rs crates/egret-engine/src/lib.rs crates/egret-engine/tests/results_csv.rs && git commit -m "feat(engine): server-side CSV export route"
```

---

## Task 7: Frontend API hooks (`sqllab.ts`)

**Files:** Create `frontend/src/api/sqllab.ts`, `frontend/src/api/__tests__/sqllab.test.ts`.

- [ ] **Step 1: failing test** `frontend/src/api/__tests__/sqllab.test.ts`
```ts
import { describe, expect, it } from "vitest";
import { csvUrl, qk } from "@/api/sqllab";

describe("sqllab api helpers", () => {
  it("builds the csv download url", () => {
    expect(csvUrl("eng", "abc")).toBe("/api/ws/eng/queries/abc/results.csv");
  });
  it("namespaces query keys by workspace", () => {
    expect(qk.saved("eng")).toEqual(["sqllab", "eng", "saved"]);
    expect(qk.schema("eng", "db1", "catalogs")).toEqual(["sqllab", "eng", "schema", "db1", "catalogs"]);
  });
});
```

- [ ] **Step 2: Run** `cd frontend && npx vitest run src/api/__tests__/sqllab.test.ts` → FAIL.

- [ ] **Step 3: `frontend/src/api/sqllab.ts`**
```ts
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch, apiJson } from "./client";

// ── types ──
export interface DatabaseListItem { id: string; name: string; engine: string }
export interface SavedQuery { id: string; database_id: string | null; name: string; sql: string; created_by: string | null; updated_at: string }
export interface SchemaColumn { name: string; type: string }
export type QueryStatus = "queued" | "running" | "finished" | "failed" | "cancelled";
export interface QueryView {
  id: string; database_id: string; status: QueryStatus;
  columns: { name: string; type: string }[] | null;
  preview: unknown[] | null; row_count: number | null; truncated: boolean; error: string | null;
}
export interface ResultsPage { columns: { name: string; type: string }[]; rows: unknown[][]; page: number; page_size: number; row_count: number; truncated: boolean }
export interface HistoryItem { id: string; database_id: string; status: QueryStatus; sql: string; row_count: number | null; submitted_at: string }

// ── query keys ──
export const qk = {
  databases: (ws: string) => ["sqllab", ws, "databases"] as const,
  saved: (ws: string) => ["sqllab", ws, "saved"] as const,
  history: (ws: string) => ["sqllab", ws, "history"] as const,
  status: (ws: string, id: string) => ["sqllab", ws, "status", id] as const,
  schema: (ws: string, db: string, level: string, ...rest: string[]) => ["sqllab", ws, "schema", db, level, ...rest] as const,
};

export const csvUrl = (ws: string, id: string) => `/api/ws/${ws}/queries/${id}/results.csv`;

// ── databases granted to the workspace ──
export function useDatabases(ws: string) {
  return useQuery<DatabaseListItem[]>({
    queryKey: qk.databases(ws),
    queryFn: () => apiJson<DatabaseListItem[]>(`/me/databases?workspace=${encodeURIComponent(ws)}`),
  });
}

// ── run / poll / results (engine API) ──
export function useSubmitQuery(ws: string) {
  return useMutation({
    mutationFn: (body: { database_id: string; sql: string }) =>
      apiJson<{ id: string; status: QueryStatus }>(`/ws/${ws}/queries`, {
        method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body),
      }),
  });
}
export function useQueryStatus(ws: string, id: string | null) {
  return useQuery<QueryView>({
    queryKey: id ? qk.status(ws, id) : ["sqllab", ws, "status", "none"],
    queryFn: () => apiJson<QueryView>(`/ws/${ws}/queries/${id}`),
    enabled: !!id,
    refetchInterval: (q) => {
      const s = q.state.data?.status;
      return s === "queued" || s === "running" ? 1000 : false;
    },
  });
}
export function fetchResults(ws: string, id: string, page: number, pageSize: number) {
  return apiJson<ResultsPage>(`/ws/${ws}/queries/${id}/results?page=${page}&page_size=${pageSize}`);
}
export function useCancelQuery(ws: string) {
  return useMutation({ mutationFn: (id: string) => apiFetch(`/ws/${ws}/queries/${id}/cancel`, { method: "POST" }) });
}
export function useHistory(ws: string) {
  return useQuery<HistoryItem[]>({ queryKey: qk.history(ws), queryFn: () => apiJson<HistoryItem[]>(`/ws/${ws}/queries`) });
}

// ── saved queries (egret-bi) ──
export function useSavedQueries(ws: string) {
  return useQuery<SavedQuery[]>({ queryKey: qk.saved(ws), queryFn: () => apiJson<SavedQuery[]>(`/ws/${ws}/sqllab/queries`) });
}
export function useSaveQuery(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: { database_id: string | null; name: string; sql: string }) =>
      apiJson<SavedQuery>(`/ws/${ws}/sqllab/queries`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) }),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.saved(ws) }),
  });
}
export function useDeleteSavedQuery(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => apiFetch(`/ws/${ws}/sqllab/queries/${id}`, { method: "DELETE" }),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.saved(ws) }),
  });
}

// ── schema browse (egret-bi), lazy + cached ──
const SCHEMA_STALE = 5 * 60 * 1000; // 5 min (Decision ②)
export function useCatalogs(ws: string, db: string | null) {
  return useQuery<string[]>({ queryKey: qk.schema(ws, db ?? "", "catalogs"), enabled: !!db, staleTime: SCHEMA_STALE,
    queryFn: () => apiJson<string[]>(`/ws/${ws}/sqllab/databases/${db}/catalogs`) });
}
export function useSchemas(ws: string, db: string | null, catalog: string) {
  return useQuery<string[]>({ queryKey: qk.schema(ws, db ?? "", "schemas", catalog), enabled: !!db, staleTime: SCHEMA_STALE,
    queryFn: () => apiJson<string[]>(`/ws/${ws}/sqllab/databases/${db}/schemas?catalog=${encodeURIComponent(catalog)}`) });
}
export function useTables(ws: string, db: string | null, catalog: string, schema: string) {
  return useQuery<string[]>({ queryKey: qk.schema(ws, db ?? "", "tables", catalog, schema), enabled: !!db && !!schema, staleTime: SCHEMA_STALE,
    queryFn: () => apiJson<string[]>(`/ws/${ws}/sqllab/databases/${db}/tables?catalog=${encodeURIComponent(catalog)}&schema=${encodeURIComponent(schema)}`) });
}
export function useColumns(ws: string, db: string | null, catalog: string, schema: string, table: string, enabled: boolean) {
  return useQuery<SchemaColumn[]>({ queryKey: qk.schema(ws, db ?? "", "columns", catalog, schema, table), enabled: !!db && enabled, staleTime: SCHEMA_STALE,
    queryFn: () => apiJson<SchemaColumn[]>(`/ws/${ws}/sqllab/databases/${db}/columns?catalog=${encodeURIComponent(catalog)}&schema=${encodeURIComponent(schema)}&table=${encodeURIComponent(table)}`) });
}
export function useRefreshSchema(ws: string) {
  const qc = useQueryClient();
  return (db: string) => qc.invalidateQueries({ queryKey: ["sqllab", ws, "schema", db] });
}
```

- [ ] **Step 4: Run** `cd frontend && npx vitest run src/api/__tests__/sqllab.test.ts` → PASS. `npm run typecheck` clean.
- [ ] **Step 5: Commit**
```bash
git add frontend/src/api/sqllab.ts frontend/src/api/__tests__/sqllab.test.ts && git commit -m "feat(sqllab): frontend API hooks"
```

---

## Task 8: SQL Lab page — editor, run, results, CSV, schema tree, autocomplete

**Files:** Create `frontend/src/components/sqllab/{SqlEditor.tsx,ResultsGrid.tsx,SchemaTree.tsx}`; Replace `frontend/src/routes/explore/SqlLab.tsx`.

> This task is large; build it component-by-component, committing once at the end. Use the existing shadcn components (`@/components/ui/{button,select,table,tabs,scroll-area}`), `react-resizable-panels`, `lucide-react` icons, and `sonner` for toasts.

- [ ] **Step 1: `frontend/src/components/sqllab/SqlEditor.tsx`** (Monaco + schema-aware autocomplete)
```tsx
import Editor, { type Monaco } from "@monaco-editor/react";
import { useRef } from "react";

export interface SchemaHint { tables: string[]; columnsByTable: Record<string, string[]> }

export function SqlEditor({ value, onChange, hint }: { value: string; onChange: (v: string) => void; hint: SchemaHint }) {
  const hintRef = useRef(hint);
  hintRef.current = hint;

  function handleMount(_editor: unknown, monaco: Monaco) {
    monaco.languages.registerCompletionItemProvider("sql", {
      provideCompletionItems: (model, position) => {
        const word = model.getWordUntilPosition(position);
        const range = { startLineNumber: position.lineNumber, endLineNumber: position.lineNumber, startColumn: word.startColumn, endColumn: word.endColumn };
        const h = hintRef.current;
        const tableItems = h.tables.map((t) => ({ label: t, kind: monaco.languages.CompletionItemKind.Struct, insertText: t, range }));
        const colItems = Object.values(h.columnsByTable).flat().map((c) => ({ label: c, kind: monaco.languages.CompletionItemKind.Field, insertText: c, range }));
        return { suggestions: [...tableItems, ...colItems] };
      },
    });
  }

  return (
    <Editor height="100%" defaultLanguage="sql" value={value} onChange={(v) => onChange(v ?? "")} onMount={handleMount}
      options={{ minimap: { enabled: false }, fontSize: 13, scrollBeyondLastLine: false }} />
  );
}
```

- [ ] **Step 2: `frontend/src/components/sqllab/ResultsGrid.tsx`**
```tsx
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Button } from "@/components/ui/button";
import type { ResultsPage } from "@/api/sqllab";

export function ResultsGrid({ page, pageIndex, onPage, csvHref, truncated }: {
  page: ResultsPage | null; pageIndex: number; onPage: (n: number) => void; csvHref: string | null; truncated: boolean;
}) {
  if (!page) return <div className="p-4 text-sm text-muted-foreground">No results yet. Run a query.</div>;
  const totalPages = Math.max(1, Math.ceil(page.row_count / page.page_size));
  return (
    <div className="flex h-full flex-col">
      {truncated && <div className="bg-yellow-100 px-3 py-1 text-xs text-yellow-900">Results were limited to {page.row_count} rows.</div>}
      <div className="flex-1 overflow-auto">
        <Table>
          <TableHeader>
            <TableRow>{page.columns.map((c) => <TableHead key={c.name}>{c.name}</TableHead>)}</TableRow>
          </TableHeader>
          <TableBody>
            {page.rows.map((row, i) => (
              <TableRow key={i}>{row.map((cell, j) => <TableCell key={j}>{cell === null ? "" : String(cell)}</TableCell>)}</TableRow>
            ))}
          </TableBody>
        </Table>
      </div>
      <div className="flex items-center gap-2 border-t p-2 text-sm">
        <Button variant="outline" size="sm" disabled={pageIndex <= 0} onClick={() => onPage(pageIndex - 1)}>Prev</Button>
        <span>Page {pageIndex + 1} / {totalPages}</span>
        <Button variant="outline" size="sm" disabled={pageIndex >= totalPages - 1} onClick={() => onPage(pageIndex + 1)}>Next</Button>
        <span className="ml-auto">{page.row_count} rows</span>
        {csvHref && <a href={csvHref} download><Button variant="outline" size="sm">Download CSV</Button></a>}
      </div>
    </div>
  );
}
```

- [ ] **Step 3: `frontend/src/components/sqllab/SchemaTree.tsx`** (lazy, click-to-insert, Refresh)
```tsx
import { useState } from "react";
import { ChevronRight, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useCatalogs, useSchemas, useTables, useRefreshSchema } from "@/api/sqllab";

export function SchemaTree({ ws, db, engine, onInsert }: { ws: string; db: string | null; engine: string | null; onInsert: (name: string) => void }) {
  const refresh = useRefreshSchema(ws);
  if (!db) return <div className="p-3 text-sm text-muted-foreground">Pick a database.</div>;
  // StarRocks has no catalog level — render schemas directly under a synthetic root.
  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center justify-between border-b p-2">
        <span className="text-xs font-medium">Schema</span>
        <Button variant="ghost" size="sm" onClick={() => refresh(db)} title="Refresh schema"><RefreshCw className="h-3 w-3" /></Button>
      </div>
      <div className="flex-1 overflow-auto p-1 text-sm">
        {engine === "trino" ? <TrinoCatalogs ws={ws} db={db} onInsert={onInsert} /> : <Schemas ws={ws} db={db} catalog="" onInsert={onInsert} />}
      </div>
    </div>
  );
}

function TrinoCatalogs({ ws, db, onInsert }: { ws: string; db: string; onInsert: (n: string) => void }) {
  const { data: cats } = useCatalogs(ws, db);
  return <>{(cats ?? []).map((c) => <Node key={c} label={c}><Schemas ws={ws} db={db} catalog={c} onInsert={onInsert} /></Node>)}</>;
}
function Schemas({ ws, db, catalog, onInsert }: { ws: string; db: string; catalog: string; onInsert: (n: string) => void }) {
  const { data: schemas } = useSchemas(ws, db, catalog);
  return <>{(schemas ?? []).map((s) => <Node key={s} label={s}><Tables ws={ws} db={db} catalog={catalog} schema={s} onInsert={onInsert} /></Node>)}</>;
}
function Tables({ ws, db, catalog, schema, onInsert }: { ws: string; db: string; catalog: string; schema: string; onInsert: (n: string) => void }) {
  const { data: tables } = useTables(ws, db, catalog, schema);
  return <>{(tables ?? []).map((t) => (
    <button key={t} className="block w-full truncate rounded px-2 py-0.5 text-left hover:bg-accent" onClick={() => onInsert(catalog ? `${catalog}.${schema}.${t}` : `${schema}.${t}`)}>{t}</button>
  ))}</>;
}
function Node({ label, children }: { label: string; children: React.ReactNode }) {
  const [open, setOpen] = useState(false);
  return (
    <div>
      <button className="flex w-full items-center gap-1 rounded px-1 py-0.5 hover:bg-accent" onClick={() => setOpen((o) => !o)}>
        <ChevronRight className={`h-3 w-3 transition-transform ${open ? "rotate-90" : ""}`} /> {label}
      </button>
      {open && <div className="ml-4">{children}</div>}
    </div>
  );
}
```

- [ ] **Step 4: Replace `frontend/src/routes/explore/SqlLab.tsx`**
```tsx
import { useMemo, useState } from "react";
import { useParams } from "react-router-dom";
import { Panel, PanelGroup, PanelResizeHandle } from "react-resizable-panels";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  csvUrl, fetchResults, useCancelQuery, useDatabases, useHistory, useQueryStatus,
  useSaveQuery, useSavedQueries, useDeleteSavedQuery, useSubmitQuery,
  type ResultsPage, type SchemaColumn,
} from "@/api/sqllab";
import { SqlEditor, type SchemaHint } from "@/components/sqllab/SqlEditor";
import { ResultsGrid } from "@/components/sqllab/ResultsGrid";
import { SchemaTree } from "@/components/sqllab/SchemaTree";

export default function SqlLab() {
  const ws = useParams().workspace!;
  const { data: databases } = useDatabases(ws);
  const [dbId, setDbId] = useState<string | null>(null);
  const db = databases?.find((d) => d.id === dbId) ?? null;
  const [sql, setSql] = useState("SELECT 1");
  const [queryId, setQueryId] = useState<string | null>(null);
  const [page, setPage] = useState(0);
  const [results, setResults] = useState<ResultsPage | null>(null);
  const PAGE = 100;

  const submit = useSubmitQuery(ws);
  const cancel = useCancelQuery(ws);
  const status = useQueryStatus(ws, queryId);
  const save = useSaveQuery(ws);

  // when the query finishes, load page 0
  const finished = status.data?.status === "finished";
  if (finished && queryId && results === null) {
    fetchResults(ws, queryId, 0, PAGE).then(setResults).catch(() => {});
  }

  async function run() {
    if (!dbId) { toast.error("Pick a database"); return; }
    setResults(null); setPage(0);
    try { const { id } = await submit.mutateAsync({ database_id: dbId, sql }); setQueryId(id); }
    catch (e) { toast.error(String(e)); }
  }
  async function goPage(n: number) {
    if (!queryId) return;
    setPage(n); setResults(await fetchResults(ws, queryId, n, PAGE));
  }
  async function onSave() {
    const name = window.prompt("Query name?"); if (!name) return;
    await save.mutateAsync({ database_id: dbId, name, sql }); toast.success("Saved");
  }

  const running = status.data?.status === "queued" || status.data?.status === "running";
  const hint: SchemaHint = useMemo(() => ({ tables: [], columnsByTable: {} }), []); // populated as the tree loads (see note)

  return (
    <PanelGroup direction="horizontal" className="h-full">
      <Panel defaultSize={20} minSize={12}>
        <div className="flex h-full flex-col border-r">
          <div className="border-b p-2">
            <Select value={dbId ?? ""} onValueChange={setDbId}>
              <SelectTrigger><SelectValue placeholder="Database" /></SelectTrigger>
              <SelectContent>{(databases ?? []).map((d) => <SelectItem key={d.id} value={d.id}>{d.name} ({d.engine})</SelectItem>)}</SelectContent>
            </Select>
          </div>
          <SchemaTree ws={ws} db={dbId} engine={db?.engine ?? null} onInsert={(n) => setSql((s) => s + n)} />
        </div>
      </Panel>
      <PanelResizeHandle className="w-1 bg-border" />
      <Panel defaultSize={80}>
        <PanelGroup direction="vertical">
          <Panel defaultSize={50} minSize={20}>
            <div className="flex h-full flex-col">
              <div className="flex items-center gap-2 border-b p-2">
                <Button size="sm" onClick={run} disabled={running}>Run</Button>
                <Button size="sm" variant="outline" disabled={!running} onClick={() => queryId && cancel.mutate(queryId)}>Cancel</Button>
                <Button size="sm" variant="outline" onClick={onSave}>Save</Button>
                {running && <span className="text-sm text-muted-foreground">running…</span>}
                {status.data?.status === "failed" && <span className="text-sm text-red-600">{status.data.error}</span>}
              </div>
              <div className="flex-1"><SqlEditor value={sql} onChange={setSql} hint={hint} /></div>
            </div>
          </Panel>
          <PanelResizeHandle className="h-1 bg-border" />
          <Panel defaultSize={50} minSize={20}>
            <Tabs defaultValue="results" className="flex h-full flex-col">
              <TabsList className="m-2 w-fit"><TabsTrigger value="results">Results</TabsTrigger><TabsTrigger value="history">History</TabsTrigger><TabsTrigger value="saved">Saved</TabsTrigger></TabsList>
              <TabsContent value="results" className="flex-1 overflow-hidden">
                <ResultsGrid page={results} pageIndex={page} onPage={goPage} truncated={status.data?.truncated ?? false}
                  csvHref={finished && queryId ? csvUrl(ws, queryId) : null} />
              </TabsContent>
              <TabsContent value="history" className="flex-1 overflow-auto"><HistoryTab ws={ws} onLoad={setSql} /></TabsContent>
              <TabsContent value="saved" className="flex-1 overflow-auto"><SavedTab ws={ws} onLoad={(s, d) => { setSql(s); if (d) setDbId(d); }} /></TabsContent>
            </Tabs>
          </Panel>
        </PanelGroup>
      </Panel>
    </PanelGroup>
  );
}

function HistoryTab({ ws, onLoad }: { ws: string; onLoad: (sql: string) => void }) {
  const { data } = useHistory(ws);
  return <div className="p-2 text-sm">{(data ?? []).map((h) => (
    <button key={h.id} className="block w-full truncate rounded px-2 py-1 text-left hover:bg-accent" onClick={() => onLoad(h.sql)} title={h.sql}>
      <span className="text-muted-foreground">[{h.status}]</span> {h.sql}
    </button>
  ))}</div>;
}

function SavedTab({ ws, onLoad }: { ws: string; onLoad: (sql: string, db: string | null) => void }) {
  const { data } = useSavedQueries(ws);
  const del = useDeleteSavedQuery(ws);
  return <div className="p-2 text-sm">{(data ?? []).map((q) => (
    <div key={q.id} className="flex items-center gap-2 rounded px-2 py-1 hover:bg-accent">
      <button className="flex-1 truncate text-left" onClick={() => onLoad(q.sql, q.database_id)} title={q.sql}>{q.name}</button>
      <button className="text-xs text-red-600" onClick={() => del.mutate(q.id)}>delete</button>
    </div>
  ))}</div>;
}
```
> **Implementer note on autocomplete population:** the `hint` passed to `SqlEditor` should be populated from the schema the user has browsed. Minimal viable approach: lift a `SchemaHint` state into `SqlLab`, and in `SchemaTree`'s `Tables`/`Node` components call an `onSchemaLoaded(tables, columnsByTable)` callback when a `useTables`/`useColumns` query resolves, accumulating into the hint. Wire that callback through `SchemaTree` props. Keep it bounded to the active catalog/schema (Decision ③). If wiring the callback cleanly proves fiddly within this task, ship the editor with keyword completion (the provider still returns the (possibly empty) table/column list) and complete the hint-population in Task 9 — note which you did. The schema-aware completion provider itself (Step 1) is the required deliverable.

- [ ] **Step 5: Verify** `cd frontend && npm run typecheck` clean; `npm run lint` clean; `npm run build` succeeds.
- [ ] **Step 6: Commit**
```bash
git add frontend/src/components/sqllab frontend/src/routes/explore/SqlLab.tsx && git commit -m "feat(sqllab): editor, run/cancel, results grid, CSV, schema tree"
```

---

## Task 9: Component tests + final verification

**Files:** Create `frontend/src/components/sqllab/__tests__/ResultsGrid.test.tsx`, `frontend/src/components/sqllab/__tests__/SqlLab.test.tsx`.

- [ ] **Step 1: `ResultsGrid.test.tsx`** (pure render — no network)
```tsx
import { render, screen } from "@testing-library/react";
import { expect, test } from "vitest";
import { ResultsGrid } from "@/components/sqllab/ResultsGrid";

test("renders columns, rows, truncation banner, and CSV link", () => {
  render(<ResultsGrid pageIndex={0} onPage={() => {}} truncated csvHref="/api/ws/eng/queries/1/results.csv"
    page={{ columns: [{ name: "n", type: "bigint" }], rows: [[1], [2]], page: 0, page_size: 100, row_count: 2, truncated: true }} />);
  expect(screen.getByText("n")).toBeInTheDocument();
  expect(screen.getByText(/Results were limited/)).toBeInTheDocument();
  expect(screen.getByText("Download CSV").closest("a")).toHaveAttribute("href", "/api/ws/eng/queries/1/results.csv");
});
```

- [ ] **Step 2: `SqlLab.test.tsx`** — the run state machine with a mocked `fetch` (submit → poll running→finished → results). Mock `window.fetch` to return: `POST /api/ws/eng/queries` → `{id:"q1",status:"queued"}`; first `GET …/queries/q1` → `{status:"running",…}`; second → `{status:"finished",columns:[{name:"n",type:"bigint"}],truncated:false,row_count:1,error:null,…}`; `GET …/queries/q1/results?...` → a `ResultsPage`. Render `<SqlLab/>` inside `MemoryRouter initialEntries={["/explore/eng/sql-lab"]}` + a `Routes`/`Route path="/explore/:workspace/sql-lab"` + `QueryClientProvider`. Pick a database (the `/me/databases` mock returns one), click **Run**, and `await waitFor(() => expect(screen.getByText("n")).toBeInTheDocument())`.
```tsx
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, test, vi } from "vitest";
import SqlLab from "@/routes/explore/SqlLab";

afterEach(() => vi.restoreAllMocks());

function mockApi() {
  let statusCalls = 0;
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    const u = String(url); const m = (init?.method ?? "GET").toUpperCase();
    const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
    if (u.includes("/me/databases")) return json([{ id: "db1", name: "trino", engine: "trino" }]);
    if (u.endsWith("/sqllab/queries")) return json([]);            // saved list
    if (u.endsWith("/queries") && m === "GET") return json([]);     // history
    if (u.endsWith("/queries") && m === "POST") return json({ id: "q1", status: "queued" });
    if (u.includes("/queries/q1/results")) return json({ columns: [{ name: "n", type: "bigint" }], rows: [[1]], page: 0, page_size: 100, row_count: 1, truncated: false });
    if (u.includes("/queries/q1")) { statusCalls++; return json(statusCalls < 2 ? { id: "q1", database_id: "db1", status: "running", columns: null, preview: null, row_count: null, truncated: false, error: null } : { id: "q1", database_id: "db1", status: "finished", columns: [{ name: "n", type: "bigint" }], preview: [[1]], row_count: 1, truncated: false, error: null }); }
    return json({});
  }));
}

test("run a query renders results", async () => {
  mockApi();
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={qc}><MemoryRouter initialEntries={["/explore/eng/sql-lab"]}>
    <Routes><Route path="/explore/:workspace/sql-lab" element={<SqlLab />} /></Routes>
  </MemoryRouter></QueryClientProvider>);
  await userEvent.click(await screen.findByText("Database")); // open select
  await userEvent.click(await screen.findByText(/trino \(trino\)/));
  await userEvent.click(screen.getByText("Run"));
  await waitFor(() => expect(screen.getByText("n")).toBeInTheDocument(), { timeout: 4000 });
});
```
> **Implementer note:** the shadcn `Select` (Radix) may need `pointer-events`/`scrollIntoView` shims under jsdom. If selecting the option is flaky, set `dbId` via a test seam or assert the run-flow with the database preselected (e.g. default the first DB when exactly one is granted — a reasonable UX improvement: auto-select when `databases.length === 1`). Prefer the auto-select UX tweak; it also improves real usage. Keep the assertion that results render after Run.

- [ ] **Step 3: Run** `cd frontend && npm test` → all pass. `npm run typecheck` + `npm run lint` clean.

- [ ] **Step 4: Backend final verification**
```bash
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings
DATABASE_URL=postgres://egret:egret@localhost:5432/egret \
EGRET_TEST_TRINO_URL=http://localhost:8090 \
EGRET_TEST_STARROCKS_URL=mysql://root@localhost:9030 \
cargo test --workspace
```
Expected: 0 failures (egret-bi saved/schema/http + engine results_csv run for real).

- [ ] **Step 5: Commit**
```bash
git add frontend/src/components/sqllab/__tests__ frontend/src/routes/explore/SqlLab.tsx && git commit -m "test(sqllab): component + run-flow tests"
```

- [ ] **Step 6: Final whole-feature review**, then **superpowers:finishing-a-development-branch**.

---

## Self-review notes (author)

- **Spec coverage:** §4.1 saved queries → Tasks 3,5; §4.2 schema introspection → Task 4; §5 migration → Task 3; §6 CSV server-side → Task 6; §7 REST surface → Tasks 5,6; §8 frontend (run state-machine, schema cache+refresh ②, autocomplete ③, results grid, CSV, saved/history) → Tasks 7,8,9; §9 wiring → Task 5; §11 security (RBAC, ownership, identifier quoting) → Tasks 4,5,6; §12 testing → every task + Task 9.
- **Decisions honored:** ① server-side CSV (Task 6); ② schema `staleTime`=5min + Refresh (Task 7 `useRefreshSchema`, Task 8 SchemaTree button); ③ autocomplete bounded to browsed schema (Task 8 hint + note); ④ tree virtualization deferred (not in any task — noted in spec).
- **Type consistency:** `SavedQuery`/`SavedQueryDto`, `SchemaColumn`, `QueryView`/`ResultsPage`/`HistoryItem`, `BiState`/`sqllab_router`, `SchemaService::{catalogs,schemas,tables,columns}`, `SavedQueryRepo::{create,list,get,update,delete,owner}`, the `qk` query-key helper and `csvUrl` are used identically across backend↔frontend tasks. Engine `rows_to_csv` matches its test.
- **No placeholders:** every step has complete code; the four implementer notes flag the spots to finalize against live APIs / jsdom (the Trino schema-test assertion, the `for_test`-vs-inline state in egret-bi http test, the autocomplete hint wiring, the Radix-Select-under-jsdom seam) — each with a concrete fallback.
- **Deferred (per spec §1):** dataset/chart/dashboard, WS streaming, schema sync, tab session-restore, multi-tab, snippets, prefix-search autocomplete endpoint, tree virtualization. Not in any task.
