# Notebook C1 — Notebook Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the notebook *document* per workspace — persist notebooks + ordered cells, expose a CRUD API on the workspace session plane, and a Monaco-based editor UI. No execution (kernels/output come in D1/E1).

**Architecture:** New backend crate `egret-notebook` mirroring `egret-engine` (model/store/routes/error/lib, `Router<NotebookState>`, `WsContext`, `require_workspace`, ownership `owned` helper). One migration `0037_notebooks.sql` (DDL + RBAC perm seed). Frontend: react-query api client + list page + editor page, reusing the existing Explore route/sidebar entry, Monaco (`sqllab/SqlEditor.tsx` pattern), and `react-markdown`.

**Tech Stack:** Rust (axum, sqlx, serde, uuid, chrono), Postgres; React 18 + Vite + TanStack Query + Monaco + react-markdown + Vitest.

**Spec:** `docs/en/specs/2026-06-29-notebook-c1-core-design.md`

**Resolved decisions:** on create, seed one empty `code` cell. RBAC: `notebook:read` → `ws_admin`,`ws_editor`,`ws_viewer`; `notebook:write` → `ws_admin`,`ws_editor`. Ownership: read = own OR public; write = owner only; **no platform_admin override**.

---

## File structure

**Backend — new crate `crates/egret-notebook/`:**
- `Cargo.toml` — mirror `egret-engine` deps (subset: no reqwest/mysql).
- `src/lib.rs` — re-exports + `pub use routes::{notebook_router, NotebookState};`
- `src/model.rs` — domain types + DTOs.
- `src/error.rs` — `NotebookApiError` (mirror `EngineApiError`).
- `src/store.rs` — `NotebookStore` + row structs + sqlx + `#[sqlx::test]`s.
- `src/routes.rs` — `notebook_router` + handlers + `owned` helper + axum tests.

**Backend — edits:**
- `migrations/0037_notebooks.sql` — DDL + perm seed (new).
- `crates/egret-metastore/src/lib.rs` — touch comment to re-embed migrations.
- `Cargo.toml` (workspace) — add `egret-notebook = { path = "crates/egret-notebook" }`.
- `crates/egret-server/Cargo.toml` — add `egret-notebook.workspace = true`.
- `crates/egret-server/src/lib.rs` — `AppState.notebook` field + merge router.
- `crates/egret-bin/src/main.rs` — build `NotebookState`, pass into `AppState`.

**Frontend:**
- `src/api/notebooks.ts` (new) — client + react-query hooks + types.
- `src/routes/explore/Notebooks.tsx` (replace placeholder) — list page.
- `src/routes/explore/NotebookPage.tsx` (new) — editor.
- `src/components/notebook/CellEditor.tsx` (new) — one cell (Monaco/markdown).
- `src/App.tsx` — add route `notebooks/:id`.
- Tests: `src/routes/explore/__tests__/Notebooks.test.tsx`,
  `src/routes/explore/__tests__/NotebookPage.test.tsx`,
  `src/api/__tests__/notebooks.test.ts` (optional).

---

## Task 1: Scaffold `egret-notebook` crate (model + error + lib)

**Files:**
- Create: `crates/egret-notebook/Cargo.toml`
- Create: `crates/egret-notebook/src/lib.rs`
- Create: `crates/egret-notebook/src/model.rs`
- Create: `crates/egret-notebook/src/error.rs`
- Modify: `Cargo.toml` (workspace members already glob `crates/*`; add the dep alias)

- [ ] **Step 1: Cargo.toml**

```toml
[package]
name = "egret-notebook"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
egret-core.workspace = true
egret-metastore.workspace = true
egret-auth.workspace = true
egret-workspace.workspace = true
axum.workspace = true
sqlx.workspace = true
serde.workspace = true
serde_json.workspace = true
uuid.workspace = true
chrono.workspace = true
thiserror.workspace = true
tracing.workspace = true

[dev-dependencies]
tower.workspace = true
http-body-util = "0.1"
egret-auth.workspace = true
egret-workspace.workspace = true
```

- [ ] **Step 2: Add workspace dep alias** in root `Cargo.toml` `[workspace.dependencies]` (after `egret-bi`):

```toml
egret-notebook = { path = "crates/egret-notebook" }
```

- [ ] **Step 3: model.rs** — domain + DTOs

```rust
//! Notebook domain types + request/response DTOs.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine { Daft, SparkConnect }
impl Engine {
    pub fn as_str(self) -> &'static str {
        match self { Engine::Daft => "daft", Engine::SparkConnect => "spark_connect" }
    }
    pub fn parse(s: &str) -> Option<Engine> {
        match s { "daft" => Some(Engine::Daft), "spark_connect" => Some(Engine::SparkConnect), _ => None }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CellType { Code, Markdown, Sql }
impl CellType {
    pub fn as_str(self) -> &'static str {
        match self { CellType::Code => "code", CellType::Markdown => "markdown", CellType::Sql => "sql" }
    }
    pub fn parse(s: &str) -> Option<CellType> {
        match s { "code" => Some(CellType::Code), "markdown" => Some(CellType::Markdown), "sql" => Some(CellType::Sql), _ => None }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Cell {
    pub id: Uuid,
    pub cell_type: String,
    pub source: String,
    pub cell_order: i32,
    pub execution_count: Option<i32>,
    pub last_output: Option<serde_json::Value>,
    pub last_execution_time_ms: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NotebookSummary {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub engine: String,
    pub is_public: bool,
    pub owner_user_id: Uuid,
    pub is_owner: bool,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NotebookDetail {
    #[serde(flatten)]
    pub summary: NotebookSummary,
    pub cells: Vec<Cell>,
}

#[derive(Debug, Deserialize)]
pub struct CreateNotebook { pub name: Option<String>, pub engine: Option<String> }

#[derive(Debug, Deserialize)]
pub struct UpdateNotebook {
    pub name: Option<String>,
    pub description: Option<String>,
    pub engine: Option<String>,
    pub is_public: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct CreateCell { pub cell_type: String, pub source: Option<String>, pub cell_order: Option<i32> }

#[derive(Debug, Deserialize)]
pub struct UpdateCell { pub cell_type: Option<String>, pub source: Option<String> }

#[derive(Debug, Deserialize)]
pub struct ReorderCells { pub order: Vec<Uuid> }
```

- [ ] **Step 4: error.rs** — mirror `egret-engine/src/error.rs`. Read that file first, then replicate its shape:

```rust
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use egret_core::Error;

#[derive(Debug)]
pub struct NotebookApiError(pub Error);

impl From<Error> for NotebookApiError { fn from(e: Error) -> Self { Self(e) } }
impl From<sqlx::Error> for NotebookApiError {
    fn from(e: sqlx::Error) -> Self { Self(Error::Internal(e.to_string())) }
}

impl IntoResponse for NotebookApiError {
    fn into_response(self) -> Response {
        let (code, msg) = match &self.0 {
            Error::NotFound(m) => (StatusCode::NOT_FOUND, m.clone()),
            Error::Forbidden => (StatusCode::FORBIDDEN, "forbidden".into()),
            Error::BadRequest(m) => (StatusCode::BAD_REQUEST, m.clone()),
            Error::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".into()),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, "internal error".into()),
        };
        (code, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}
```

> If `egret_core::Error` variant names differ (e.g. `BadRequest`), adjust to the actual enum — confirm by reading `crates/egret-core/src` for `pub enum Error`.

- [ ] **Step 5: lib.rs**

```rust
//! Per-workspace notebook documents: CRUD persistence + REST API (no execution).
pub mod error;
pub mod model;
pub mod store;
pub mod routes;

pub use routes::{notebook_router, NotebookState};
pub use store::NotebookStore;
```

- [ ] **Step 6: Compile**

Run: `cargo build -p egret-notebook`
Expected: fails only on missing `store`/`routes` modules → create empty stubs (`pub struct NotebookStore;` etc.) is unnecessary; instead proceed to Task 3/4 which create them. For this task, temporarily comment out `pub mod store; pub mod routes;` and the `pub use`s, build to verify model+error compile, then restore.

Run: `cargo build -p egret-notebook` (with store/routes commented)
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/egret-notebook Cargo.toml
git commit -m "feat(notebook): scaffold egret-notebook crate (model + error)"
```

---

## Task 2: Migration `0037_notebooks.sql` + re-embed

**Files:**
- Create: `migrations/0037_notebooks.sql`
- Modify: `crates/egret-metastore/src/lib.rs` (touch comment)

- [ ] **Step 1: Write the migration**

```sql
-- Notebook C1: per-workspace notebook documents + ordered cells, + RBAC perms.
CREATE TABLE notebooks (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id  uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    owner_user_id uuid NOT NULL REFERENCES users(id),
    name          text NOT NULL DEFAULT 'Untitled',
    description   text NOT NULL DEFAULT '',
    engine        text NOT NULL DEFAULT 'daft',
    is_public     boolean NOT NULL DEFAULT false,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX idx_notebooks_ws_owner ON notebooks(workspace_id, owner_user_id);

CREATE TABLE notebook_cells (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    notebook_id  uuid NOT NULL REFERENCES notebooks(id) ON DELETE CASCADE,
    cell_type    text NOT NULL DEFAULT 'code',
    source       text NOT NULL DEFAULT '',
    cell_order   integer NOT NULL DEFAULT 0,
    execution_count        integer,
    last_output            jsonb,
    last_execution_time_ms integer,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX idx_notebook_cells_notebook ON notebook_cells(notebook_id, cell_order);

-- RBAC: notebook:read for all workspace roles; notebook:write for admin + editor.
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'notebook:read' FROM roles
WHERE scope='workspace' AND name IN ('ws_admin','ws_editor','ws_viewer')
ON CONFLICT (role_id, permission) DO NOTHING;

INSERT INTO role_permissions (role_id, permission)
SELECT id, 'notebook:write' FROM roles
WHERE scope='workspace' AND name IN ('ws_admin','ws_editor')
ON CONFLICT (role_id, permission) DO NOTHING;
```

- [ ] **Step 2: Touch metastore to re-embed** — edit the header comment in `crates/egret-metastore/src/lib.rs` (line ~2) to mention `0037_notebooks`. (Per memory `egret-sqlx-migration-staleness`, the embedded migrator only re-embeds when this file changes.)

- [ ] **Step 3: Verify migration applies on a fresh test DB**

Run: `sqlx database drop -y --database-url "$DATABASE_URL" && sqlx database create --database-url "$DATABASE_URL" && sqlx migrate run --source migrations --database-url "$DATABASE_URL"`
Expected: applies through `0037` with no error.

- [ ] **Step 4: Commit**

```bash
git add migrations/0037_notebooks.sql crates/egret-metastore/src/lib.rs
git commit -m "feat(notebook): migration 0037 — notebooks/cells tables + RBAC perms"
```

---

## Task 3: `NotebookStore` (persistence) — TDD

**Files:**
- Create: `crates/egret-notebook/src/store.rs`

Read `crates/egret-engine/src/store.rs` first for the row-struct + `Db` + `#[sqlx::test]` conventions (note: egret uses `sqlx::test(migrations = "...")` or a pool fixture — match exactly what egret-engine does).

- [ ] **Step 1: Write failing tests** (append to `store.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use egret_metastore::Db;

    // Helper: insert a workspace + user, return (ws_id, user_id). Mirror how
    // egret-engine tests seed prerequisites (read egret-engine/src/store.rs tests).
    async fn seed(db: &Db) -> (uuid::Uuid, uuid::Uuid) { /* INSERT workspaces + users, RETURN ids */ unimplemented!() }

    #[sqlx::test(migrations = "../../migrations")]
    async fn create_get_list_delete(pool: sqlx::PgPool) {
        let db = Db::from_pool(pool);
        let store = NotebookStore::new(db.clone());
        let (ws, user) = seed(&db).await;

        let id = store.create(ws, user, "My NB", "daft").await.unwrap();
        // create seeds exactly one empty code cell
        let detail = store.get_with_cells(id).await.unwrap().unwrap();
        assert_eq!(detail.cells.len(), 1);
        assert_eq!(detail.cells[0].cell_type, "code");

        let list = store.list_visible(ws, user).await.unwrap();
        assert_eq!(list.len(), 1);

        store.delete(id).await.unwrap();
        assert!(store.get_with_cells(id).await.unwrap().is_none());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn list_visibility_owner_and_public(pool: sqlx::PgPool) {
        let db = Db::from_pool(pool);
        let store = NotebookStore::new(db.clone());
        let (ws, alice) = seed(&db).await;
        let bob = /* insert a 2nd user */ unimplemented!();

        let priv_nb = store.create(ws, alice, "priv", "daft").await.unwrap();
        let pub_nb = store.create(ws, alice, "pub", "daft").await.unwrap();
        store.update(pub_nb, &crate::model::UpdateNotebook{ name:None, description:None, engine:None, is_public:Some(true) }).await.unwrap();

        // bob sees only the public one
        let bob_list = store.list_visible(ws, bob).await.unwrap();
        assert_eq!(bob_list.len(), 1);
        assert_eq!(bob_list[0].id, pub_nb);
        let _ = priv_nb;
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn cell_crud_and_reorder(pool: sqlx::PgPool) {
        let db = Db::from_pool(pool);
        let store = NotebookStore::new(db.clone());
        let (ws, user) = seed(&db).await;
        let nb = store.create(ws, user, "nb", "daft").await.unwrap();

        let c2 = store.add_cell(nb, "markdown", "# hi", 1).await.unwrap();
        let detail = store.get_with_cells(nb).await.unwrap().unwrap();
        let c1 = detail.cells[0].id;
        assert_eq!(detail.cells.len(), 2);

        store.reorder(nb, &[c2, c1]).await.unwrap();
        let after = store.get_with_cells(nb).await.unwrap().unwrap();
        assert_eq!(after.cells[0].id, c2);

        store.delete_cell(nb, c1).await.unwrap();
        assert_eq!(store.get_with_cells(nb).await.unwrap().unwrap().cells.len(), 1);
    }
}
```

- [ ] **Step 2: Run — verify it fails** `cargo test -p egret-notebook` → FAIL (NotebookStore undefined).

- [ ] **Step 3: Implement `store.rs`** (above the tests module). Mirror `QueryStore` style: a `NotebookStore { db: Db }`, `NotebookRow`/`CellRow` `FromRow` structs with `into_*` mappers, parameterized queries only.

```rust
use egret_metastore::Db;
use uuid::Uuid;
use crate::error::NotebookApiError;
use crate::model::{Cell, NotebookSummary, NotebookDetail, UpdateNotebook, UpdateCell};

#[derive(Clone)]
pub struct NotebookStore { db: Db }

impl NotebookStore {
    pub fn new(db: Db) -> Self { Self { db } }

    pub async fn create(&self, ws: Uuid, owner: Uuid, name: &str, engine: &str) -> Result<Uuid, NotebookApiError> {
        let mut tx = self.db.pool.begin().await?;
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO notebooks (workspace_id, owner_user_id, name, engine) VALUES ($1,$2,$3,$4) RETURNING id")
            .bind(ws).bind(owner).bind(name).bind(engine)
            .fetch_one(&mut *tx).await?;
        // seed one empty code cell
        sqlx::query("INSERT INTO notebook_cells (notebook_id, cell_type, source, cell_order) VALUES ($1,'code','',0)")
            .bind(id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(id)
    }

    pub async fn list_visible(&self, ws: Uuid, user: Uuid) -> Result<Vec<NotebookSummary>, NotebookApiError> {
        // ORDER BY updated_at DESC; is_owner = (owner_user_id = $2)
        // SELECT ... WHERE workspace_id=$1 AND (owner_user_id=$2 OR is_public)
        todo!("map rows → NotebookSummary")
    }

    /// Fetch by id WITHOUT scoping — callers (routes) enforce ws + ownership.
    pub async fn get_summary(&self, id: Uuid) -> Result<Option<(NotebookSummary, Uuid)>, NotebookApiError> {
        // returns (summary, workspace_id) so the route can check workspace match
        todo!()
    }

    pub async fn get_with_cells(&self, id: Uuid) -> Result<Option<NotebookDetail>, NotebookApiError> {
        // get_summary + SELECT cells ORDER BY cell_order, created_at
        todo!()
    }

    pub async fn update(&self, id: Uuid, p: &UpdateNotebook) -> Result<(), NotebookApiError> {
        // COALESCE-style partial update; set updated_at=now()
        todo!()
    }

    pub async fn delete(&self, id: Uuid) -> Result<(), NotebookApiError> {
        sqlx::query("DELETE FROM notebooks WHERE id=$1").bind(id).execute(&self.db.pool).await?;
        Ok(())
    }

    pub async fn add_cell(&self, nb: Uuid, ty: &str, source: &str, order: i32) -> Result<Uuid, NotebookApiError> {
        let id = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO notebook_cells (notebook_id, cell_type, source, cell_order) VALUES ($1,$2,$3,$4) RETURNING id")
            .bind(nb).bind(ty).bind(source).bind(order).fetch_one(&self.db.pool).await?;
        self.touch(nb).await?;
        Ok(id)
    }

    pub async fn update_cell(&self, nb: Uuid, cid: Uuid, p: &UpdateCell) -> Result<(), NotebookApiError> {
        // partial update WHERE id=$1 AND notebook_id=$2 (defends nested IDOR at SQL too)
        todo!()
    }

    pub async fn delete_cell(&self, nb: Uuid, cid: Uuid) -> Result<(), NotebookApiError> {
        sqlx::query("DELETE FROM notebook_cells WHERE id=$1 AND notebook_id=$2").bind(cid).bind(nb)
            .execute(&self.db.pool).await?;
        self.touch(nb).await?;
        Ok(())
    }

    pub async fn reorder(&self, nb: Uuid, order: &[Uuid]) -> Result<(), NotebookApiError> {
        let mut tx = self.db.pool.begin().await?;
        for (i, cid) in order.iter().enumerate() {
            sqlx::query("UPDATE notebook_cells SET cell_order=$1, updated_at=now() WHERE id=$2 AND notebook_id=$3")
                .bind(i as i32).bind(cid).bind(nb).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        self.touch(nb).await?;
        Ok(())
    }

    async fn touch(&self, nb: Uuid) -> Result<(), NotebookApiError> {
        sqlx::query("UPDATE notebooks SET updated_at=now() WHERE id=$1").bind(nb).execute(&self.db.pool).await?;
        Ok(())
    }
}
```

> Replace every `todo!()` with the parameterized query + row mapping before finishing the task. Confirm `Db` exposes `pool` and a `from_pool` constructor by reading `crates/egret-metastore/src/lib.rs`; if the test helper differs, match egret-engine's test setup exactly.

- [ ] **Step 4: Run tests** `cargo test -p egret-notebook` → PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/egret-notebook/src/store.rs
git commit -m "feat(notebook): NotebookStore CRUD + cell ops (TDD)"
```

---

## Task 4: `notebook_router` + handlers + ownership — TDD

**Files:**
- Create: `crates/egret-notebook/src/routes.rs`

Read `crates/egret-engine/src/routes.rs` first (EngineState, FromRef impls, `WsContext`, `require_workspace`, `owned`).

- [ ] **Step 1: Write failing axum tests** (tower `oneshot`, mirror egret-engine route tests). Cover:
  - create → 201; get → 200 with 1 seeded cell.
  - non-owner GET private → 403; after set public → 200.
  - **platform_admin** GET another's private notebook → still 403 (no override).
  - cross-workspace GET (notebook of ws A under ws B path) → 404.
  - PATCH/DELETE by non-owner → 403.
  - cell add/update/delete/reorder happy path; cell of notebook X requested under notebook Y → 404.
  - unknown `engine`/`cell_type` → 400.

  (Build the app via `notebook_router().with_state(test_state)`. Construct a `NotebookState` with real `NotebookStore` over the test pool + the same `SessionService`/`WorkspaceRepo`/`Authorizer` constructors egret-engine's tests use — read those tests to copy the auth/session seeding helper.)

- [ ] **Step 2: Run — verify fail** `cargo test -p egret-notebook` → FAIL.

- [ ] **Step 3: Implement `routes.rs`**

```rust
use std::sync::Arc;
use axum::extract::{Path, State};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use axum::http::StatusCode;
use egret_auth::session::SessionService;
use egret_core::{Error, WorkspaceContext};
use egret_workspace::context::WsContext;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use uuid::Uuid;

use crate::error::NotebookApiError;
use crate::model::*;
use crate::store::NotebookStore;

const READ: &str = "notebook:read";
const WRITE: &str = "notebook:write";

#[derive(Clone)]
pub struct NotebookState {
    pub store: NotebookStore,
    pub sessions: SessionService,
    pub workspaces: WorkspaceRepo,
    pub authz: Authorizer,
}
impl axum::extract::FromRef<NotebookState> for SessionService { fn from_ref(s: &NotebookState) -> Self { s.sessions.clone() } }
impl axum::extract::FromRef<NotebookState> for WorkspaceRepo { fn from_ref(s: &NotebookState) -> Self { s.workspaces.clone() } }
impl axum::extract::FromRef<NotebookState> for Authorizer { fn from_ref(s: &NotebookState) -> Self { s.authz.clone() } }

pub fn notebook_router() -> Router<NotebookState> {
    Router::new()
        .route("/ws/{workspace}/notebooks", get(list).post(create))
        .route("/ws/{workspace}/notebooks/{id}", get(get_one).patch(update).delete(remove))
        .route("/ws/{workspace}/notebooks/{id}/cells", post(add_cell))
        .route("/ws/{workspace}/notebooks/{id}/cells/order", put(reorder))
        .route("/ws/{workspace}/notebooks/{id}/cells/{cid}", axum::routing::patch(update_cell).delete(delete_cell))
}

// read-scope: returns (summary, owner, is_public). 404 if cross-ws.
async fn read_scoped(st: &NotebookState, ctx: &WorkspaceContext, id: Uuid) -> Result<NotebookSummary, NotebookApiError> {
    let (mut summary, ws) = st.store.get_summary(id).await?.ok_or_else(|| Error::NotFound("notebook".into()))?;
    if ws != ctx.workspace_id.0 { return Err(Error::NotFound("notebook".into()).into()); }
    let me = ctx.principal.user_id.0;
    if summary.owner_user_id != me && !summary.is_public { return Err(Error::Forbidden.into()); }
    summary.is_owner = summary.owner_user_id == me;
    Ok(summary)
}

// write-scope: owner only, NO admin override.
async fn write_scoped(st: &NotebookState, ctx: &WorkspaceContext, id: Uuid) -> Result<(), NotebookApiError> {
    let (summary, ws) = st.store.get_summary(id).await?.ok_or_else(|| Error::NotFound("notebook".into()))?;
    if ws != ctx.workspace_id.0 { return Err(Error::NotFound("notebook".into()).into()); }
    if summary.owner_user_id != ctx.principal.user_id.0 { return Err(Error::Forbidden.into()); }
    Ok(())
}

async fn list(State(st): State<NotebookState>, WsContext(ctx): WsContext) -> Result<Json<Vec<NotebookSummary>>, NotebookApiError> {
    st.authz.require_workspace(&ctx, READ).await.map_err(Error::from)?;
    let nbs = st.store.list_visible(ctx.workspace_id.0, ctx.principal.user_id.0).await?;
    Ok(Json(nbs))
}

async fn create(State(st): State<NotebookState>, WsContext(ctx): WsContext, Json(b): Json<CreateNotebook>) -> Result<(StatusCode, Json<NotebookDetail>), NotebookApiError> {
    st.authz.require_workspace(&ctx, WRITE).await.map_err(Error::from)?;
    let engine = b.engine.as_deref().unwrap_or("daft");
    Engine::parse(engine).ok_or_else(|| Error::BadRequest("invalid engine".into()))?;
    let name = b.name.as_deref().unwrap_or("Untitled");
    let id = st.store.create(ctx.workspace_id.0, ctx.principal.user_id.0, name, engine).await?;
    let detail = st.store.get_with_cells(id).await?.ok_or_else(|| Error::NotFound("notebook".into()))?;
    Ok((StatusCode::CREATED, Json(detail)))
}

async fn get_one(State(st): State<NotebookState>, WsContext(ctx): WsContext, Path((_ws, id)): Path<(String, Uuid)>) -> Result<Json<NotebookDetail>, NotebookApiError> {
    st.authz.require_workspace(&ctx, READ).await.map_err(Error::from)?;
    let _ = read_scoped(&st, &ctx, id).await?;
    let detail = st.store.get_with_cells(id).await?.ok_or_else(|| Error::NotFound("notebook".into()))?;
    Ok(Json(detail))
}

async fn update(State(st): State<NotebookState>, WsContext(ctx): WsContext, Path((_ws, id)): Path<(String, Uuid)>, Json(b): Json<UpdateNotebook>) -> Result<Json<NotebookDetail>, NotebookApiError> {
    st.authz.require_workspace(&ctx, WRITE).await.map_err(Error::from)?;
    write_scoped(&st, &ctx, id).await?;
    if let Some(e) = &b.engine { Engine::parse(e).ok_or_else(|| Error::BadRequest("invalid engine".into()))?; }
    st.store.update(id, &b).await?;
    let detail = st.store.get_with_cells(id).await?.ok_or_else(|| Error::NotFound("notebook".into()))?;
    Ok(Json(detail))
}

async fn remove(State(st): State<NotebookState>, WsContext(ctx): WsContext, Path((_ws, id)): Path<(String, Uuid)>) -> Result<StatusCode, NotebookApiError> {
    st.authz.require_workspace(&ctx, WRITE).await.map_err(Error::from)?;
    write_scoped(&st, &ctx, id).await?;
    st.store.delete(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn add_cell(State(st): State<NotebookState>, WsContext(ctx): WsContext, Path((_ws, id)): Path<(String, Uuid)>, Json(b): Json<CreateCell>) -> Result<(StatusCode, Json<NotebookDetail>), NotebookApiError> {
    st.authz.require_workspace(&ctx, WRITE).await.map_err(Error::from)?;
    write_scoped(&st, &ctx, id).await?;
    CellType::parse(&b.cell_type).ok_or_else(|| Error::BadRequest("invalid cell_type".into()))?;
    st.store.add_cell(id, &b.cell_type, b.source.as_deref().unwrap_or(""), b.cell_order.unwrap_or(0)).await?;
    let detail = st.store.get_with_cells(id).await?.ok_or_else(|| Error::NotFound("notebook".into()))?;
    Ok((StatusCode::CREATED, Json(detail)))
}

async fn update_cell(State(st): State<NotebookState>, WsContext(ctx): WsContext, Path((_ws, id, cid)): Path<(String, Uuid, Uuid)>, Json(b): Json<UpdateCell>) -> Result<StatusCode, NotebookApiError> {
    st.authz.require_workspace(&ctx, WRITE).await.map_err(Error::from)?;
    write_scoped(&st, &ctx, id).await?;
    if let Some(t) = &b.cell_type { CellType::parse(t).ok_or_else(|| Error::BadRequest("invalid cell_type".into()))?; }
    st.store.update_cell(id, cid, &b).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_cell(State(st): State<NotebookState>, WsContext(ctx): WsContext, Path((_ws, id, cid)): Path<(String, Uuid, Uuid)>) -> Result<StatusCode, NotebookApiError> {
    st.authz.require_workspace(&ctx, WRITE).await.map_err(Error::from)?;
    write_scoped(&st, &ctx, id).await?;
    st.store.delete_cell(id, cid).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn reorder(State(st): State<NotebookState>, WsContext(ctx): WsContext, Path((_ws, id)): Path<(String, Uuid)>, Json(b): Json<ReorderCells>) -> Result<StatusCode, NotebookApiError> {
    st.authz.require_workspace(&ctx, WRITE).await.map_err(Error::from)?;
    write_scoped(&st, &ctx, id).await?;
    st.store.reorder(id, &b.order).await?;
    Ok(StatusCode::NO_CONTENT)
}
```

> Verify the exact signatures of `Authorizer::require_workspace`, `WsContext`, `WorkspaceContext.principal.user_id`, and `egret_core::Error` variants against `egret-engine/src/routes.rs` — adapt `.map_err(Error::from)?` to however egret-engine maps the authz error (it uses `.map_err(EngineApiError::from)`). The `_ = Arc` import line may be unused — drop unused imports to satisfy clippy.

- [ ] **Step 4: Run tests** `cargo test -p egret-notebook` → PASS. Then re-enable `pub mod store; pub mod routes;` + the `pub use`s in `lib.rs` (from Task 1 Step 6) and `cargo build -p egret-notebook`.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-notebook/src
git commit -m "feat(notebook): notebook_router + handlers + ownership (TDD)"
```

---

## Task 5: Wire `egret-notebook` into the server

**Files:**
- Modify: `crates/egret-server/Cargo.toml`
- Modify: `crates/egret-server/src/lib.rs`
- Modify: `crates/egret-bin/src/main.rs`

- [ ] **Step 1: server dep** — add to `crates/egret-server/Cargo.toml` deps: `egret-notebook.workspace = true`.

- [ ] **Step 2: AppState field** — in `crates/egret-server/src/lib.rs`, add to `struct AppState` (after `bi`): `pub notebook: Option<egret_notebook::NotebookState>,` and set `notebook: None` in the no-db constructor.

- [ ] **Step 3: merge router** — in `build_router`, after the `bi` block, mirror it:

```rust
if let Some(notebook) = state.notebook.clone() {
    let group = Router::new()
        .nest("/api", egret_notebook::notebook_router())
        .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
        .with_state(notebook);
    router = router.merge(group);
}
```

- [ ] **Step 4: build NotebookState in egret-bin** — in `crates/egret-bin/src/main.rs`, after the `BiState` block (~line 500), construct it reusing the same `sessions`, `workspaces`, `authz` already built for the engine/bi states:

```rust
let notebook = Some(egret_notebook::NotebookState {
    store: egret_notebook::NotebookStore::new(db.clone()),
    sessions: sessions.clone(),
    workspaces: workspaces.clone(),
    authz: authz.clone(),
});
```

and add `notebook,` to the `AppState { … }` initializer. (Read main.rs ~440–520 to use the exact local variable names for sessions/workspaces/authz.)

- [ ] **Step 5: build** `cargo build -p egret-bin` → PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-server crates/egret-bin
git commit -m "feat(notebook): wire NotebookState into server + composition root"
```

---

## Task 6: Frontend api client `notebooks.ts`

**Files:**
- Create: `frontend/src/api/notebooks.ts`

Read `frontend/src/api/sqllab.ts` for `apiJson`/`apiFetch`, `qk` keys, and hook conventions.

- [ ] **Step 1: Implement**

```ts
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiJson } from "./client";

export type Engine = "daft" | "spark_connect";
export type CellType = "code" | "markdown" | "sql";

export interface Cell {
  id: string; cell_type: CellType; source: string; cell_order: number;
  execution_count: number | null; last_output: unknown | null; last_execution_time_ms: number | null;
}
export interface NotebookSummary {
  id: string; name: string; description: string; engine: Engine;
  is_public: boolean; owner_user_id: string; is_owner: boolean; updated_at: string;
}
export interface NotebookDetail extends NotebookSummary { cells: Cell[] }

export const qk = {
  list: (ws: string) => ["notebooks", ws] as const,
  detail: (ws: string, id: string) => ["notebooks", ws, id] as const,
};

const base = (ws: string) => `/ws/${encodeURIComponent(ws)}/notebooks`;

export function useNotebooks(ws: string) {
  return useQuery<NotebookSummary[]>({ queryKey: qk.list(ws), queryFn: () => apiJson(`${base(ws)}`) });
}
export function useNotebook(ws: string, id: string | null) {
  return useQuery<NotebookDetail>({
    queryKey: id ? qk.detail(ws, id) : ["notebooks", ws, "none"],
    queryFn: () => apiJson(`${base(ws)}/${id}`), enabled: !!id,
  });
}
function jsonBody(method: string, body?: unknown) {
  return { method, headers: { "content-type": "application/json" }, body: body ? JSON.stringify(body) : undefined };
}
export function useCreateNotebook(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (b: { name?: string; engine?: Engine }) => apiJson<NotebookDetail>(base(ws), jsonBody("POST", b)),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.list(ws) }),
  });
}
export function useUpdateNotebook(ws: string, id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (b: Partial<Pick<NotebookSummary, "name" | "description" | "engine" | "is_public">>) =>
      apiJson<NotebookDetail>(`${base(ws)}/${id}`, jsonBody("PATCH", b)),
    onSuccess: () => { qc.invalidateQueries({ queryKey: qk.detail(ws, id) }); qc.invalidateQueries({ queryKey: qk.list(ws) }); },
  });
}
export function useDeleteNotebook(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => apiJson(`${base(ws)}/${id}`, jsonBody("DELETE")),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.list(ws) }),
  });
}
export function useAddCell(ws: string, id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (b: { cell_type: CellType; source?: string; cell_order?: number }) => apiJson<NotebookDetail>(`${base(ws)}/${id}/cells`, jsonBody("POST", b)),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.detail(ws, id) }),
  });
}
export function useUpdateCell(ws: string, id: string) {
  return useMutation({
    mutationFn: (a: { cid: string; cell_type?: CellType; source?: string }) =>
      apiJson(`${base(ws)}/${id}/cells/${a.cid}`, jsonBody("PATCH", { cell_type: a.cell_type, source: a.source })),
  });
}
export function useDeleteCell(ws: string, id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (cid: string) => apiJson(`${base(ws)}/${id}/cells/${cid}`, jsonBody("DELETE")),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.detail(ws, id) }),
  });
}
export function useReorderCells(ws: string, id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (order: string[]) => apiJson(`${base(ws)}/${id}/cells/order`, jsonBody("PUT", { order })),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.detail(ws, id) }),
  });
}
```

> Confirm `apiJson` returns parsed JSON and tolerates 204 (no body) for DELETE/PUT/PATCH — check `api/client.ts`; if it throws on empty body, add an `apiFetch` (no-parse) variant for those calls.

- [ ] **Step 2: typecheck** `cd frontend && npm run typecheck` → PASS.
- [ ] **Step 3: Commit** `git add frontend/src/api/notebooks.ts && git commit -m "feat(notebook): frontend api client"`

---

## Task 7: Frontend list page `Notebooks.tsx` — TDD

**Files:**
- Modify: `frontend/src/routes/explore/Notebooks.tsx`
- Create: `frontend/src/routes/explore/__tests__/Notebooks.test.tsx`

Read `frontend/src/routes/explore/DatabasesPage.tsx` + its test for the page/dialog/typed-confirm/mock patterns.

- [ ] **Step 1: Failing test** — mirror `DatabasesPage.test.tsx`: mock `@/api/notebooks`; assert (a) list renders names + a "Public" badge on public ones; (b) empty state "No notebooks yet."; (c) clicking New → create dialog → submit calls `useCreateNotebook` with `{name, engine}`; (d) delete typed-confirm calls `useDeleteNotebook`.

- [ ] **Step 2: Run → fail** `npm run test -- Notebooks` → FAIL.

- [ ] **Step 3: Implement the list page** — `useParams` for `workspace`; `useNotebooks(ws)`; grid of cards (name, engine badge, "Public" badge when `is_public`, updated_at); New button → dialog (name input + engine select daft/spark_connect) → `useCreateNotebook` then `navigate(`/explore/${ws}/notebooks/${created.id}`)`; per-card menu: open (Link to `notebooks/:id`), delete (typed-confirm dialog like DatabasesPage). Empty state. Loading skeleton.

- [ ] **Step 4: Run → pass** `npm run test -- Notebooks` → PASS.
- [ ] **Step 5: Commit** `git add frontend/src/routes/explore/Notebooks.tsx frontend/src/routes/explore/__tests__/Notebooks.test.tsx && git commit -m "feat(notebook): notebook list page"`

---

## Task 8: Frontend editor `NotebookPage.tsx` + `CellEditor.tsx` + route — TDD

**Files:**
- Create: `frontend/src/routes/explore/NotebookPage.tsx`
- Create: `frontend/src/components/notebook/CellEditor.tsx`
- Modify: `frontend/src/App.tsx` (add route)
- Create: `frontend/src/routes/explore/__tests__/NotebookPage.test.tsx`

- [ ] **Step 1: Add route** — in `App.tsx`, beside `<Route path="notebooks" .../>` add `<Route path="notebooks/:id" element={<NotebookPage />} />` and import it.

- [ ] **Step 2: Failing test** — mock `@/api/notebooks` to return a 2-cell notebook (1 code, 1 markdown). Assert: (a) header shows the notebook name + engine select + public toggle; (b) a code cell renders an editor (stub Monaco — see Step 3 note); (c) editing a code cell's source then waiting the debounce calls `useUpdateCell` with the new source; (d) "Add cell" calls `useAddCell`; (e) markdown cell renders its rendered HTML (e.g. an `<h1>` from `# hi`); (f) delete-cell button calls `useDeleteCell`. Mock the Monaco component: `vi.mock("@monaco-editor/react", () => ({ default: ({ value, onChange }) => <textarea data-testid="monaco" value={value} onChange={(e)=>onChange(e.target.value)} /> }))`.

- [ ] **Step 3: Implement `CellEditor.tsx`** — props `{ cell, onChangeSource, onChangeType, onDelete, onMoveUp, onMoveDown, runDisabled }`. Code/SQL → `@monaco-editor/react` `<Editor height="..." language={cell.cell_type==="sql"?"sql":"python"} value={cell.source} onChange={...} />`. Markdown → a textarea + a `<ReactMarkdown remarkPlugins={[remarkGfm]}>` preview (see `components/chart/dashboard/TextCard.tsx`). A small toolbar: type select, move up/down, delete, and a **disabled** Run button (`title="Execution coming soon"`).

- [ ] **Step 4: Implement `NotebookPage.tsx`** — `useParams` ws+id; `useNotebook(ws, id)`; local cell-source state seeded from server; header (rename via `useUpdateNotebook` on blur, engine select, public toggle); render cells ordered; **autosave**: a `useDebouncedCallback`-style effect (or a small `setTimeout` ref, ~600ms) per cell calling `useUpdateCell`. Buttons: Add cell (`useAddCell`), move/delete (reuse `useReorderCells`/`useDeleteCell`). Back link to the list.

> No new debounce dep needed — implement a tiny debounce with `useRef<number>` + `setTimeout`/`clearTimeout`, or check if the repo already has one (grep `debounce` in `frontend/src`).

- [ ] **Step 5: Run → pass** `npm run test -- NotebookPage` → PASS.
- [ ] **Step 6: typecheck + build** `npm run typecheck && npm run build` → PASS.
- [ ] **Step 7: Commit** `git add frontend/src && git commit -m "feat(notebook): notebook editor page + cell editor"`

---

## Task 9: Full local CI gate + live smoke

Per memory `egret-run-full-ci-local-before-merge` (GitHub Actions is out of quota) and `egret-cargo-fmt-ci`.

- [ ] **Step 1: Backend**

Run: `cargo fmt --all --check` → PASS (run `cargo fmt --all` if not).
Run: `cargo clippy --workspace --all-targets -- -D warnings` → PASS.
Run: `cargo test --workspace` → PASS.

- [ ] **Step 2: Frontend**

Run: `cd frontend && npm run lint && npm run typecheck && npm run test && npm run build` → all PASS.

- [ ] **Step 3: Live smoke** (per memory `egret-frontend-viz-verify`): start docker + `cargo run -- serve` + `npm run dev`, login `admin@egret.local / Admin12345!`, open Explore → Notebooks: create a notebook → it opens with one code cell → type Python in the code cell, add a Markdown cell, see the preview → reload, edits persisted → delete the notebook. Confirm 0 console errors. The Run button is visibly disabled.

- [ ] **Step 4: Commit any fmt/lint fixups**, then proceed to `superpowers:finishing-a-development-branch`.

---

## Self-review notes (author)
- Spec coverage: data model (T2), API + ownership incl. no-admin-override (T3/T4 tests), list visibility (T3), nested IDOR (T4 + SQL `AND notebook_id=` guards), frontend list+editor+autosave+disabled-run (T7/T8), tests + local CI (T9). ✔
- Type consistency: `NotebookStore`, `NotebookState`, `NotebookDetail`/`NotebookSummary`/`Cell`, hook names align across backend↔frontend (`is_owner`, `cell_order`, `cell_type`, `engine`). ✔
- Known verify-points called out inline: exact `egret_core::Error` variants, `Db::from_pool`, `Authorizer::require_workspace` signature, `apiJson` 204 handling, egret-bin local var names — each task says "read file X first" rather than guessing.
