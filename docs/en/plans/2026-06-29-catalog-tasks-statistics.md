# Catalog — warehouse Tasks + Statistics tabs — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add two read-only tabs — **Statistics** (table/view counts over time) and **Tasks** (the GC/maintenance task list) — to the catalog warehouse detail, backed by the lakekeeper management API.

**Architecture:** Two new GET routes in `egret-catalog-engine/src/browse.rs` under the existing `{wid}` scope, gated `catalog:read` via the existing `authorized_warehouse` helper (extended to also carry the resolved lakekeeper UUID). The statistics route calls `lakekeeper::api::management::v1::warehouse::Service::get_warehouse_statistics`; the tasks route calls `lakekeeper::api::management::v1::tasks::Service::list_tasks` — both dispatched on the egret-specialized `ApiServer` type, exactly as `provision_ws.rs` already calls `create_warehouse`. The frontend adds two TanStack-Query hooks and two `Tabs` panels in `WarehouseDetail.tsx`.

**Tech Stack:** Rust (axum, lakekeeper embedded lib, sqlx/Postgres, MinIO) · React 18 + Vite + Tailwind v4 + shadcn/Radix + TanStack Query + Vitest.

**Reference (spec):** `docs/en/specs/2026-06-29-catalog-tasks-statistics-design.md`.

---

## Background facts (verified against the pinned lakekeeper checkout)

- **Dispatch type** (define locally in `browse.rs`, mirroring `provision_ws.rs:61`):
  `type EngineApiServer = ApiServer<PostgresBackend, EgretAuthorizer, SecretsState>;`
- **Statistics** — `management::v1::warehouse::Service::get_warehouse_statistics(warehouse_id: WarehouseId, query: GetWarehouseStatisticsQuery, context: ApiContext<EngineState>, request_metadata) -> Result<WarehouseStatisticsResponse>`.
  - `WarehouseStatisticsResponse { warehouse_ident: Uuid, stats: Vec<WarehouseStatistics>, next_page_token: Option<String> }`.
  - `WarehouseStatistics { timestamp: DateTime<Utc>, number_of_tables: i64, number_of_views: i64, updated_at: DateTime<Utc> }`.
  - `GetWarehouseStatisticsQuery { page_token: PageToken, page_size: Option<i64> }` — **no `Default` derive**; construct `{ page_token: PageToken::Empty, page_size: None }` (`PageToken` already imported in `browse.rs:54`).
  - Stats are computed lazily (hourly) — a fresh warehouse returns `stats: []`. That is a valid 200, not an error.
- **Tasks** — `management::v1::tasks::Service::list_tasks(warehouse_id: WarehouseId, query: ListTasksRequest, context: ApiContext<EngineState>, request_metadata) -> Result<ListTasksResponse>`.
  - `ListTasksResponse { tasks: Vec<WarehouseTaskInfo>, next_page_token: Option<String> }`.
  - `ListTasksRequest` **derives `Default`** — use `ListTasksRequest::default()`.
  - `WarehouseTaskInfo` fields used: `task_id: TaskId` (newtype over `Uuid`, has `Display`; private field → use `.to_string()`), `queue_name: TaskQueueName` (newtype over `String`, `Deref<Target=str>` → `.to_string()`), `status: TaskStatus`, `entity_name: Option<Vec<String>>`, `scheduled_for: DateTime<Utc>`, `attempt: i32`, `progress: f32`, `created_at: DateTime<Utc>`. (No `last_error` field exists — omit it, per the spec's "map only fields that exist".)
  - `TaskStatus` derives `Serialize` with `#[serde(rename_all = "SCREAMING_SNAKE_CASE")]` → serializes as `RUNNING | SCHEDULED | STOPPING | CANCELLED | SUCCESS | FAILED`.
- **Error mapping:** management calls return `lakekeeper::api::Result<T>` (`Err = IcebergErrorResponse`), and `BrowseApiError: From<IcebergErrorResponse>` (`browse.rs:244`) — so `.await?` maps errors exactly like `table_detail` does.
- **Context value:** `st.engine` is `ApiContext<EngineState>` (`BrowseState.engine`, `browse.rs:78`); pass `st.engine.clone()` as the `context` arg, exactly as `table_detail` passes it to `load_table`.
- **Ownership / IDOR:** `authorized_warehouse` already (a) gates `catalog:read` and (b) resolves `{wid}` → lakekeeper UUID workspace-scoped (cross-workspace → 404). We extend its return value `Resolved` with the resolved `lakekeeper_id: Uuid` so the new handlers get the `WarehouseId` without a second DB hit.

---

## Task 1: Backend — statistics + tasks routes

**Files:**
- Modify: `crates/egret-catalog-engine/src/browse.rs` (imports; `Resolved` struct + its 2 construction sites; two handlers; two route registrations)
- Test: `crates/egret-catalog-engine/tests/tasks_stats_api.rs` (new)

### Step 1: Write the failing integration test

Create `crates/egret-catalog-engine/tests/tasks_stats_api.rs`. It mirrors `tests/table_detail_api.rs` (same `setup()` / `add_member` / `into_json` fixture shape — copy them verbatim from that file), then exercises the two new routes. Full file:

```rust
//! Session-authenticated catalog **statistics + tasks** tests (catalog
//! sub-epic 4). DB + MinIO + lakekeeper gated; self-skips without `DATABASE_URL`.
//!
//! Proves the two read-only management-backed routes under the warehouse `{wid}`
//! scope:
//!   - GET …/{wid}/statistics → 200 `{ "entries": [...] }` (empty is valid for a
//!     fresh warehouse — lakekeeper computes stats lazily/hourly),
//!   - GET …/{wid}/tasks       → 200 `[...]` (usually empty — GC tasks only
//!     appear after table expiry/purge),
//!   - both gate `catalog:read` and are IDOR-safe: a `{wid}` from another
//!     workspace → 404.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   cargo test -p egret-catalog-engine --test tasks_stats_api -- --test-threads=1

use axum::body::Body;
use axum::extract::Request;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_catalog::service::CatalogStore;
use egret_catalog_engine::{
    browse_router, build_state, ensure_bootstrapped, provision_workspace_warehouse,
    run_lakekeeper_migrations, BrowseState,
};
use egret_core::ids::{UserId, WorkspaceId};
use egret_metastore::Db;
use egret_storage::StorageConfig;
use egret_workspace::members::MemberRepo;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use http_body_util::BodyExt;
use lakekeeper::api::RequestMetadata;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

const SYSTEM_USER: &str = "egret-system";

fn dev_storage_config() -> StorageConfig {
    StorageConfig {
        endpoint: Some("http://localhost:9000".to_string()),
        region: "local-01".to_string(),
        bucket: "warehouse".to_string(),
        access_key: "egret".to_string(),
        secret_key: "egretsecret".to_string(),
        path_style: true,
    }
}

struct Fixture {
    app: axum::Router,
    sessions: SessionService,
    users: UserRepo,
    members: MemberRepo,
    workspace_id: WorkspaceId,
    owner: UserId,
    slug: String,
    /// The egret warehouse-ROW id (`{wid}` path segment).
    wid: Uuid,
}

async fn inject_request_metadata(mut request: Request, next: Next) -> Response {
    request
        .extensions_mut()
        .insert(RequestMetadata::new_lakekeeper_internal(Uuid::now_v7()));
    next.run(request).await
}

async fn into_json(resp: Response) -> (StatusCode, serde_json::Value) {
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(
        |_| serde_json::json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }),
    );
    (status, json)
}

async fn setup() -> Option<Fixture> {
    let dsn = std::env::var("DATABASE_URL").ok()?;

    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    run_lakekeeper_migrations(&dsn)
        .await
        .expect("lakekeeper migrations");
    let (engine_state, _registry) = build_state(&dsn, SYSTEM_USER)
        .await
        .expect("build engine state");
    ensure_bootstrapped(engine_state.clone(), SYSTEM_USER)
        .await
        .expect("bootstrap");

    let store = CatalogStore::new(db.clone());
    let users = UserRepo::new(db.clone());
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );

    let suffix = Uuid::now_v7().simple().to_string();
    let owner = users
        .insert_local(&format!("tsowner-{suffix}@egret.test"), "TS Owner", "x")
        .await
        .expect("seed owner");
    let slug = format!("tsws{suffix}");
    let ws = WorkspaceRepo::new(db.clone())
        .create(&slug, "Tasks-Stats Test", owner)
        .await
        .expect("seed workspace");
    let workspace_id: WorkspaceId = ws.id;

    provision_workspace_warehouse(
        engine_state.clone(),
        &store,
        workspace_id.0,
        &slug,
        &dev_storage_config(),
        SYSTEM_USER,
    )
    .await
    .expect("provision warehouse (needs MinIO reachable + writable `warehouse` bucket)");

    let wid = store
        .get_warehouse_by_name(workspace_id.0, "default")
        .await
        .expect("default warehouse row")
        .id;

    let crypto =
        egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
    let secret_store = egret_secrets::SecretStore::new(db.clone(), Some(crypto));
    let app = browse_router().with_state(BrowseState {
        engine: engine_state,
        store: store.clone(),
        workspace_secrets: egret_secrets::WorkspaceSecretStore::new(db.clone(), secret_store),
        system_user: SYSTEM_USER.into(),
        sessions: sessions.clone(),
        workspaces: WorkspaceRepo::new(db.clone()),
        authz: Authorizer::new(db.clone()),
    });

    Some(Fixture {
        app,
        sessions,
        users,
        members: MemberRepo::new(db.clone()),
        workspace_id,
        owner,
        slug,
        wid,
    })
}

async fn add_member(f: &Fixture, email_tag: &str, role: &str) -> String {
    let email = format!("{email_tag}_{}@egret.local", Uuid::new_v4().simple());
    let uid = f
        .users
        .insert_local(&email, "Test", "x")
        .await
        .expect("insert user");
    f.members
        .add(f.workspace_id, "user", uid.0, role, f.owner)
        .await
        .expect("seed member");
    f.sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .expect("create session")
        .token
}

async fn get(f: &Fixture, token: &str, uri: &str) -> (StatusCode, serde_json::Value) {
    let resp = f
        .app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .uri(uri)
                .header("Cookie", format!("egret_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    into_json(resp).await
}

#[tokio::test]
async fn statistics_returns_entries_array_for_a_reader() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "viewer", "ws_viewer").await;

    let uri = format!("/ws/{}/catalog/warehouses/{}/statistics", f.slug, f.wid);
    let (status, body) = get(&f, &token, &uri).await;
    assert_eq!(status, StatusCode::OK, "statistics; body={body}");
    // A fresh warehouse may have an empty series — assert the SHAPE, tolerant of empty.
    assert!(body.get("entries").and_then(|e| e.as_array()).is_some(), "entries array; body={body}");
}

#[tokio::test]
async fn tasks_returns_list_for_a_reader() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "viewer", "ws_viewer").await;

    let uri = format!("/ws/{}/catalog/warehouses/{}/tasks", f.slug, f.wid);
    let (status, body) = get(&f, &token, &uri).await;
    assert_eq!(status, StatusCode::OK, "tasks; body={body}");
    assert!(body.is_array(), "tasks is a JSON array; body={body}");
}

#[tokio::test]
async fn cross_workspace_wid_is_404_on_both_routes() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "viewer", "ws_viewer").await;

    // A random {wid} that does not belong to this workspace → 404 (IDOR-safe).
    let alien = Uuid::now_v7();
    for path in ["statistics", "tasks"] {
        let uri = format!("/ws/{}/catalog/warehouses/{}/{}", f.slug, alien, path);
        let (status, body) = get(&f, &token, &uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path} cross-ws; body={body}");
    }
}

#[tokio::test]
async fn non_member_is_gated() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    // A logged-in user who is NOT a member of this workspace.
    let outsider = f
        .users
        .insert_local(&format!("outsider_{}@egret.test", Uuid::new_v4().simple()), "Out", "x")
        .await
        .expect("insert outsider");
    let token = f
        .sessions
        .create(outsider, "test", "127.0.0.1")
        .await
        .expect("session")
        .token;

    let uri = format!("/ws/{}/catalog/warehouses/{}/statistics", f.slug, f.wid);
    let (status, _body) = get(&f, &token, &uri).await;
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND,
        "non-member gated, got {status}"
    );
}
```

### Step 2: Run the test to verify it fails

Run: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-catalog-engine --test tasks_stats_api -- --test-threads=1`
Expected: **compile error** — the routes don't exist yet (404s would also fail the 200 assertions). This confirms the test targets unbuilt behavior.

### Step 3: Extend `Resolved` to carry the lakekeeper UUID

In `browse.rs`, change the `Resolved` struct (line ~402) and both construction sites (in `authorized_warehouse` ~428 and `authorized_warehouse_admin` ~457).

Struct:
```rust
struct Resolved {
    prefix: Prefix,
    /// The resolved lakekeeper warehouse UUID (ownership-checked). Needed by the
    /// management-API reads (statistics / tasks) that take a `WarehouseId`.
    lakekeeper_id: Uuid,
    request_metadata: RequestMetadata,
}
```

Both `Ok(Resolved { ... })` blocks already compute `let lk_id = ...`. Add `lakekeeper_id: lk_id,` to each:
```rust
    Ok(Resolved {
        // lakekeeper's default `prefix_template = "{warehouse_id}"`.
        prefix: Prefix::from(lk_id.to_string()),
        lakekeeper_id: lk_id,
        request_metadata,
    })
```
(and the identical addition in `authorized_warehouse_admin`).

### Step 4: Add the management-API imports

In `browse.rs`, alongside the existing `lakekeeper::api::...` imports (after line 62), add:
```rust
use lakekeeper::api::management::v1::tasks::{
    ListTasksRequest, Service as TasksMgmtService, TaskStatus,
};
use lakekeeper::api::management::v1::warehouse::Service as WarehouseMgmtService;
use lakekeeper::api::management::v1::{ApiServer, GetWarehouseStatisticsQuery};
use lakekeeper::service::WarehouseId;
use lakekeeper_storage_postgres::{PostgresBackend, SecretsState};
```
And add the authorizer import next to the other `crate::` imports (after line 66):
```rust
use crate::authorizer::EgretAuthorizer;
```
Then define the dispatch alias near the top of the handlers region (after the `Resolved` helpers, before the warehouse-management section ~line 474):
```rust
/// `ApiServer` specialized to egret's concrete backends — the type whose
/// management `Service` impls carry `get_warehouse_statistics` / `list_tasks`.
/// Mirrors `provision_ws::EngineApiServer`.
type EngineApiServer = ApiServer<PostgresBackend, EgretAuthorizer, SecretsState>;
```

> If `lakekeeper_storage_postgres` is not already a dependency of `egret-catalog-engine`, it is (provision_ws.rs imports it). No `Cargo.toml` change is needed. Verify with `rtk proxy grep -n lakekeeper_storage_postgres crates/egret-catalog-engine/Cargo.toml` — if absent, add the same dependency line `provision_ws.rs` relies on.

### Step 5: Add the two handlers

Add near `table_detail` (e.g. after it, ~line 1200) in `browse.rs`:

```rust
// ---------------------------------------------------------------------------
// Read-only management-API tabs (catalog sub-epic 4), under the `{wid}` scope.
// Gated `catalog:read` via `authorized_warehouse` (IDOR-safe `{wid}` → lakekeeper
// UUID). Both are pure reads — no mutation, no `control_tasks`.
// ---------------------------------------------------------------------------

/// One statistics sample (a point in lakekeeper's lazily-computed hourly series).
#[derive(Serialize)]
struct StatEntryDto {
    timestamp: DateTime<Utc>,
    number_of_tables: i64,
    number_of_views: i64,
    updated_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct StatisticsDto {
    entries: Vec<StatEntryDto>,
}

/// GET /ws/{ws}/catalog/warehouses/{wid}/statistics
async fn warehouse_statistics(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Path((_ws, wid)): Path<(String, Uuid)>,
) -> Result<Json<StatisticsDto>, BrowseApiError> {
    let r = authorized_warehouse(&st, &ctx, wid).await?;
    let resp = <EngineApiServer as WarehouseMgmtService<_, _, _>>::get_warehouse_statistics(
        WarehouseId::from(r.lakekeeper_id),
        GetWarehouseStatisticsQuery {
            page_token: PageToken::Empty,
            page_size: None,
        },
        st.engine.clone(),
        r.request_metadata,
    )
    .await?;

    let mut entries: Vec<StatEntryDto> = resp
        .stats
        .into_iter()
        .map(|s| StatEntryDto {
            timestamp: s.timestamp,
            number_of_tables: s.number_of_tables,
            number_of_views: s.number_of_views,
            updated_at: s.updated_at,
        })
        .collect();
    // Newest first.
    entries.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    Ok(Json(StatisticsDto { entries }))
}

/// A GC/maintenance task row as exposed to the UI. `task_type` is the lakekeeper
/// queue name (e.g. `tabular_expiration`); `status` serializes as lakekeeper's
/// SCREAMING_SNAKE_CASE (`RUNNING`/`SCHEDULED`/`SUCCESS`/`FAILED`/…).
#[derive(Serialize)]
struct TaskDto {
    task_id: String,
    task_type: String,
    status: TaskStatus,
    entity_name: Option<String>,
    created_at: DateTime<Utc>,
    scheduled_for: DateTime<Utc>,
    attempt: i32,
    progress: f32,
}

/// GET /ws/{ws}/catalog/warehouses/{wid}/tasks
async fn warehouse_tasks(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Path((_ws, wid)): Path<(String, Uuid)>,
) -> Result<Json<Vec<TaskDto>>, BrowseApiError> {
    let r = authorized_warehouse(&st, &ctx, wid).await?;
    let resp = <EngineApiServer as TasksMgmtService<_, _, _>>::list_tasks(
        WarehouseId::from(r.lakekeeper_id),
        ListTasksRequest::default(),
        st.engine.clone(),
        r.request_metadata,
    )
    .await?;

    let tasks = resp
        .tasks
        .into_iter()
        .map(|t| TaskDto {
            task_id: t.task_id.to_string(),
            task_type: t.queue_name.to_string(),
            status: t.status,
            entity_name: t.entity_name.map(|levels| levels.join(".")),
            created_at: t.created_at,
            scheduled_for: t.scheduled_for,
            attempt: t.attempt,
            progress: t.progress,
        })
        .collect();
    Ok(Json(tasks))
}
```

### Step 6: Register the two routes

In `browse_router()` (after the table-detail route, ~line 148), add:
```rust
        // Read-only management tabs (catalog sub-epic 4), gated `catalog:read`.
        .route(
            "/ws/{workspace}/catalog/warehouses/{wid}/statistics",
            get(warehouse_statistics),
        )
        .route(
            "/ws/{workspace}/catalog/warehouses/{wid}/tasks",
            get(warehouse_tasks),
        )
```

### Step 7: Run the test to verify it passes

Run: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-catalog-engine --test tasks_stats_api -- --test-threads=1`
Expected: **PASS** (4 tests; or "skip: DATABASE_URL not set" lines if the DB is down — in CI/local make sure the egret_test DB + MinIO are up).

### Step 8: Format + lint

Run: `cargo fmt --all` then `cargo clippy -p egret-catalog-engine --all-targets -- -D warnings`
Expected: clean (no diff from fmt, no clippy warnings).

### Step 9: Commit

```bash
git add crates/egret-catalog-engine/src/browse.rs crates/egret-catalog-engine/tests/tasks_stats_api.rs
git commit -m "feat(catalog): add read-only warehouse statistics + tasks routes

Two GET routes under the {wid} scope, gated catalog:read via
authorized_warehouse (extended to carry the resolved lakekeeper UUID):
- /statistics  -> get_warehouse_statistics -> { entries: [...] }
- /tasks       -> list_tasks               -> [...]
Both are pure management-API reads (no control_tasks). IDOR-safe."
```

---

## Task 2: Frontend — Statistics + Tasks tabs

**Files:**
- Modify: `frontend/src/api/catalog.ts` (types + two hooks + query keys)
- Modify: `frontend/src/routes/explore/catalog/WarehouseDetail.tsx` (two `Tabs` panels + their bodies)
- Test: `frontend/src/routes/explore/catalog/__tests__/WarehouseDetail.test.tsx` (new)

### Step 1: Write the failing Vitest test

Create `frontend/src/routes/explore/catalog/__tests__/WarehouseDetail.test.tsx`. It mocks `@/api/catalog` (mirroring `CatalogBrowse.test.tsx`'s mock approach) and asserts the two tabs render counts/rows and their empty states. Because `WarehouseDetail` reads `useParams`, render it inside a `MemoryRouter` at a `/explore/:workspace/catalog/:wid` route.

```tsx
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route } from "react-router-dom";

const warehouseMock = vi.fn();
const statsMock = vi.fn();
const tasksMock = vi.fn();

vi.mock("@/api/catalog", () => ({
  useWarehouse: (ws: string, wid: string) => warehouseMock(ws, wid),
  useRenameWarehouse: () => ({ mutateAsync: vi.fn(), isPending: false }),
  useWarehouseStatistics: (ws: string, wid: string) => statsMock(ws, wid),
  useWarehouseTasks: (ws: string, wid: string) => tasksMock(ws, wid),
}));

// CatalogBrowse / CatalogAccess are heavy children unrelated to these tabs.
vi.mock("../CatalogBrowse", () => ({ default: () => <div>browse-stub</div> }));
vi.mock("../CatalogAccess", () => ({ default: () => <div>access-stub</div> }));

import WarehouseDetail from "../WarehouseDetail";

function renderAt() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/explore/eng/catalog/wh-1"]}>
        <Routes>
          <Route path="/explore/:workspace/catalog/:wid" element={<WarehouseDetail />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  warehouseMock.mockReset();
  statsMock.mockReset();
  tasksMock.mockReset();
  warehouseMock.mockReturnValue({
    data: {
      id: "wh-1",
      name: "Analytics",
      storage_conn_id: "c1",
      storage_conn_name: "minio",
      created_at: "2026-06-01T00:00:00Z",
    },
    isLoading: false,
    isError: false,
  });
  statsMock.mockReturnValue({
    data: {
      entries: [
        {
          timestamp: "2026-06-29T10:00:00Z",
          number_of_tables: 7,
          number_of_views: 2,
          updated_at: "2026-06-29T10:05:00Z",
        },
      ],
    },
    isLoading: false,
    isError: false,
  });
  tasksMock.mockReturnValue({
    data: [
      {
        task_id: "t-1",
        task_type: "tabular_expiration",
        status: "SUCCESS",
        entity_name: "sale.orders",
        created_at: "2026-06-29T09:00:00Z",
        scheduled_for: "2026-06-29T09:00:00Z",
        attempt: 1,
        progress: 1.0,
      },
    ],
    isLoading: false,
    isError: false,
  });
});

describe("WarehouseDetail tabs", () => {
  it("renders the Statistics tab with the latest counts", async () => {
    const user = userEvent.setup();
    renderAt();
    await user.click(screen.getByRole("tab", { name: "Statistics" }));
    expect(statsMock).toHaveBeenCalledWith("eng", "wh-1");
    expect(screen.getByText("7")).toBeInTheDocument(); // tables
    expect(screen.getByText("2")).toBeInTheDocument(); // views
  });

  it("renders the Tasks tab with a row + status badge", async () => {
    const user = userEvent.setup();
    renderAt();
    await user.click(screen.getByRole("tab", { name: "Tasks" }));
    expect(tasksMock).toHaveBeenCalledWith("eng", "wh-1");
    expect(screen.getByText("tabular_expiration")).toBeInTheDocument();
    expect(screen.getByText("sale.orders")).toBeInTheDocument();
    expect(screen.getByText("SUCCESS")).toBeInTheDocument();
  });

  it("shows empty states when there is no data", async () => {
    statsMock.mockReturnValue({ data: { entries: [] }, isLoading: false, isError: false });
    tasksMock.mockReturnValue({ data: [], isLoading: false, isError: false });
    const user = userEvent.setup();
    renderAt();

    await user.click(screen.getByRole("tab", { name: "Statistics" }));
    expect(screen.getByText(/No statistics yet/)).toBeInTheDocument();

    await user.click(screen.getByRole("tab", { name: "Tasks" }));
    expect(screen.getByText("No tasks.")).toBeInTheDocument();
  });
});
```

### Step 2: Run the test to verify it fails

Run: `cd frontend && npx vitest run src/routes/explore/catalog/__tests__/WarehouseDetail.test.tsx`
Expected: FAIL — `useWarehouseStatistics`/`useWarehouseTasks` don't exist and the tabs aren't rendered.

### Step 3: Add the API types + hooks

In `frontend/src/api/catalog.ts`, add to `catalogKeys` (inside the object literal):
```ts
  statistics: (ws: string, wid: string) => ["catalog", ws, wid, "statistics"] as const,
  tasks: (ws: string, wid: string) => ["catalog", ws, wid, "tasks"] as const,
```
Add the types + hooks (e.g. after the per-warehouse browse section, before the access-tokens section):
```ts
// ── Per-warehouse statistics + tasks (read-only management tabs) ────────────

export interface WarehouseStatEntry {
  timestamp: string;
  number_of_tables: number;
  number_of_views: number;
  updated_at: string;
}

export interface WarehouseTask {
  task_id: string;
  task_type: string;
  status: string;
  entity_name: string | null;
  created_at: string;
  scheduled_for: string;
  attempt: number;
  progress: number;
}

export function useWarehouseStatistics(ws: string, wid: string) {
  return useQuery<{ entries: WarehouseStatEntry[] }, ApiError>({
    queryKey: catalogKeys.statistics(ws, wid),
    queryFn: () =>
      apiJson<{ entries: WarehouseStatEntry[] }>(
        `/ws/${ws}/catalog/warehouses/${wid}/statistics`,
      ),
    retry: false,
  });
}

export function useWarehouseTasks(ws: string, wid: string) {
  return useQuery<WarehouseTask[], ApiError>({
    queryKey: catalogKeys.tasks(ws, wid),
    queryFn: () =>
      apiJson<WarehouseTask[]>(`/ws/${ws}/catalog/warehouses/${wid}/tasks`),
    retry: false,
  });
}
```

### Step 4: Add the two tabs + their bodies in `WarehouseDetail.tsx`

Update imports at the top:
```tsx
import {
  type Warehouse,
  type WarehouseTask,
  useRenameWarehouse,
  useWarehouse,
  useWarehouseStatistics,
  useWarehouseTasks,
} from "@/api/catalog";
import { Badge } from "@/components/ui/badge";
```

Add the two `TabsTrigger`s after the `details` trigger (line ~94):
```tsx
          <TabsTrigger value="statistics">Statistics</TabsTrigger>
          <TabsTrigger value="tasks">Tasks</TabsTrigger>
```

Add the two `TabsContent` panels after the `details` content (line ~109):
```tsx
        <TabsContent value="statistics">
          <StatisticsTab ws={ws} wid={wid} />
        </TabsContent>

        <TabsContent value="tasks">
          <TasksTab ws={ws} wid={wid} />
        </TabsContent>
```

Add the two components (and a small status-badge helper) at the bottom of the file:
```tsx
function StatisticsTab({ ws, wid }: { ws: string; wid: string }) {
  const stats = useWarehouseStatistics(ws, wid);

  if (stats.isLoading) {
    return (
      <div className="p-4">
        <ListSkeleton rows={2} cols={2} />
      </div>
    );
  }

  const entries = stats.data?.entries ?? [];
  if (entries.length === 0) {
    return (
      <div className="p-4 text-sm text-muted-foreground">
        No statistics yet — lakekeeper computes these hourly.
      </div>
    );
  }

  const latest = entries[0];
  const history = entries.slice(1);

  return (
    <div className="flex flex-col gap-6 p-4">
      <div className="flex flex-wrap items-baseline gap-x-8 gap-y-2">
        <Stat label="Tables" value={latest.number_of_tables} />
        <Stat label="Views" value={latest.number_of_views} />
        <span className="text-sm text-muted-foreground">
          as of <RelativeTime iso={latest.timestamp} />
        </span>
      </div>

      {history.length > 0 && (
        <table className="max-w-xl text-sm">
          <thead>
            <tr className="border-b text-left text-muted-foreground">
              <th className="py-1.5 pr-6 font-medium">Time</th>
              <th className="py-1.5 pr-6 font-medium">Tables</th>
              <th className="py-1.5 font-medium">Views</th>
            </tr>
          </thead>
          <tbody>
            {history.map((e) => (
              <tr key={e.timestamp} className="border-b last:border-0">
                <td className="py-1.5 pr-6">
                  <RelativeTime iso={e.timestamp} />
                </td>
                <td className="py-1.5 pr-6">{e.number_of_tables}</td>
                <td className="py-1.5">{e.number_of_views}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

function Stat({ label, value }: { label: string; value: number }) {
  return (
    <div className="flex flex-col">
      <span className="text-2xl font-semibold tabular-nums">{value}</span>
      <span className="text-xs text-muted-foreground">{label}</span>
    </div>
  );
}

function TasksTab({ ws, wid }: { ws: string; wid: string }) {
  const tasks = useWarehouseTasks(ws, wid);

  if (tasks.isLoading) {
    return (
      <div className="p-4">
        <ListSkeleton rows={3} cols={4} />
      </div>
    );
  }

  const rows = tasks.data ?? [];
  if (rows.length === 0) {
    return <div className="p-4 text-sm text-muted-foreground">No tasks.</div>;
  }

  return (
    <div className="p-4">
      <table className="w-full text-sm">
        <thead>
          <tr className="border-b text-left text-muted-foreground">
            <th className="py-1.5 pr-6 font-medium">Type</th>
            <th className="py-1.5 pr-6 font-medium">Status</th>
            <th className="py-1.5 pr-6 font-medium">Entity</th>
            <th className="py-1.5 pr-6 font-medium">Created</th>
            <th className="py-1.5 font-medium">Attempt</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((t) => (
            <tr key={t.task_id} className="border-b last:border-0">
              <td className="py-1.5 pr-6 font-mono text-xs">{t.task_type}</td>
              <td className="py-1.5 pr-6">
                <Badge variant={statusVariant(t.status)}>{t.status}</Badge>
              </td>
              <td className="py-1.5 pr-6">
                {t.entity_name ?? <span className="text-muted-foreground/50">—</span>}
              </td>
              <td className="py-1.5 pr-6">
                <RelativeTime iso={t.created_at} />
              </td>
              <td className="py-1.5">{t.attempt}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function statusVariant(
  status: WarehouseTask["status"],
): "default" | "secondary" | "destructive" | "outline" {
  switch (status) {
    case "RUNNING":
      return "default";
    case "SUCCESS":
      return "secondary";
    case "FAILED":
      return "destructive";
    default: // SCHEDULED / STOPPING / CANCELLED
      return "outline";
  }
}
```

> `RelativeTime` and `ListSkeleton` are already imported in this file. `Badge` is the only new component import.

### Step 5: Run the test to verify it passes

Run: `cd frontend && npx vitest run src/routes/explore/catalog/__tests__/WarehouseDetail.test.tsx`
Expected: PASS (3 tests).

### Step 6: Typecheck + lint + full FE test + build

Run (from `frontend/`):
```bash
npm run lint
npm run typecheck   # or: npx tsc --noEmit
npx vitest run
npm run build
```
Expected: all clean.

### Step 7: Commit

```bash
git add frontend/src/api/catalog.ts frontend/src/routes/explore/catalog/WarehouseDetail.tsx frontend/src/routes/explore/catalog/__tests__/WarehouseDetail.test.tsx
git commit -m "feat(catalog): add Statistics + Tasks tabs to the warehouse detail

Two read-only tabs backed by the new management routes:
- Statistics: latest table/view counts + a compact history table, with an
  empty state for the lazy/hourly series,
- Tasks: a Type/Status/Entity/Created/Attempt table with a status Badge and
  a 'No tasks.' empty state."
```

---

## Final verification (before merge — per the project discipline)

1. **Full local CI** (GitHub Actions is out of quota — run everything locally):
   - Backend: `cargo fmt --all --check` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo test -p egret-catalog-engine` (with `DATABASE_URL` + MinIO up).
   - Frontend (`frontend/`): `npm run lint` · `npm run typecheck` · `npx vitest run` · `npm run build`.
2. **Rebuild + restart the backend** (`cargo build -p egret-bin --bin egret`, then restart `egret serve` with the catalog env) — a stale binary serves the OLD API shape and the hot-reloaded FE would call a missing route. (See `[[egret-seed-catalog-table]]`.)
3. **Live Playwright smoke:** log in (`admin@egret.local` / `Admin12345!`), open a catalog warehouse (the seeded `sale.orders` table exists in warehouse `123`), click **Statistics** (counts or the empty state) and **Tasks** (rows or "No tasks.") — confirm no console errors.
4. **Admin squash-merge** with the configured identity (`hoangquocvietbk2.13@gmail.com`).
5. **Memory update:** mark catalog sub-epic 4 DONE in `egret-three-layer-management-model.md`.

---

## Self-review

- **Spec coverage:** statistics route (§1) → Task 1 Step 5 `warehouse_statistics`; tasks route (§1) → `warehouse_tasks`; `catalog:read` gate + IDOR 404 → reuse `authorized_warehouse` + cross-ws test; FE hooks (§2) → Task 2 Step 3; Statistics tab latest+history+empty (§2) → `StatisticsTab`; Tasks tab table+badge+empty (§2) → `TasksTab` + `statusVariant`; testing (§3) → `tasks_stats_api.rs` + `WarehouseDetail.test.tsx`; out-of-scope items (control_tasks, size bytes, permissions, chart) → not implemented. ✅
- **Placeholders:** none — every step has exact code/commands.
- **Type consistency:** BE `TaskDto.status: TaskStatus` serializes to the SCREAMING_SNAKE_CASE strings the FE `statusVariant` switches on (`RUNNING`/`SUCCESS`/`FAILED`/default). FE `WarehouseStatEntry`/`WarehouseTask` fields match the BE `StatEntryDto`/`TaskDto` serde field names exactly. `useWarehouseStatistics` returns `{ entries }`; the BE returns `{ entries: [...] }`. ✅
