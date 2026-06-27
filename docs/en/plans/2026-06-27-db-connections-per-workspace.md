# DB connections → per-workspace Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the global register-then-grant database model with per-workspace self-service DB connections (workspace-admin gated, MANAGE zone), without touching the secret model.

**Architecture:** Add `workspace_id` to the existing `databases` table (keep the PK so `queries.database_id` and all engine resolve-by-id paths keep working); make `DatabaseRepo` workspace-scoped; expose a new `/ws/{ws}/databases` router gated `workspace:manage_members` (mirror the engine `/ws/{ws}/...` pattern and the storage browse handlers); drop the old admin/me routers + grants; add a `DatabasesPage` mirroring `StoragePage`; switch the SQL Lab database picker to the workspace endpoint.

**Tech Stack:** Rust (axum, sqlx 0.8, Postgres), egret-database / egret-engine / egret-server crates; React 18 + Vite + TanStack Query + shadcn; Vitest.

**Spec:** `docs/en/specs/2026-06-27-db-connections-per-workspace-design.md`. **Branch:** `feat/db-per-workspace` (off main, already created).

**Key facts (verified):**
- Migration head is `migrations/0030_workspace_storages.sql` → new file is `0031_databases_per_workspace.sql`.
- `WsContext` extractor: `egret_workspace::context::WsContext` → `egret_core::WorkspaceContext { workspace_id, principal, … }`; it resolves `/ws/{workspace}` slug→id automatically (used by `egret-engine`).
- Workspace-admin gate: `authz.require_workspace(&ctx, "workspace:manage_members")` (same gate Storage uses).
- The engine `authorize()` (`egret-engine/src/routes.rs:64`) already calls `repo.list_for_workspace(ctx.workspace_id.0)` and checks membership — so once `list_for_workspace` filters by `workspace_id`, the engine authorize becomes ownership-based **with no engine change**. `job.rs::execute` resolves the connection via the **unscoped** `get(id)` + `open_secret(id)` AFTER authorize, so those two methods stay unscoped.
- Only FE consumer of `/me/databases` is `frontend/src/api/sqllab.ts:32`.
- Dev data: 587 `databases`, 2 granted (1 ws each, no fan-out), ~585 ungranted test fixtures, 7128 `queries`.

---

## Task 1: Migration + workspace-scoped repo

**Files:**
- Create: `migrations/0031_databases_per_workspace.sql`
- Modify: `crates/egret-database/src/repo.rs`
- Delete: `crates/egret-database/src/grants.rs`
- Modify: `crates/egret-database/src/lib.rs` (drop `mod grants;`)
- Modify: `crates/egret-metastore/src/lib.rs` (whitespace touch to force migrator re-embed)
- Test: `crates/egret-database/tests/repo_workspace_scope.rs` (new; DB-gated, self-skips without `DATABASE_URL`)

- [ ] **Step 1: Write the migration**

`migrations/0031_databases_per_workspace.sql`:
```sql
-- DB connections become per-workspace (ADR-19): each database belongs to one
-- workspace instead of being globally registered and granted.
ALTER TABLE databases
    ADD COLUMN workspace_id uuid REFERENCES workspaces(id) ON DELETE CASCADE;

CREATE INDEX databases_workspace_id_idx ON databases(workspace_id);

-- Backfill: each granted database adopts its workspace. The data shows no
-- fan-out (every granted db has exactly one grant), so this is 1:1.
UPDATE databases d
SET workspace_id = g.workspace_id
FROM workspace_database_grant g
WHERE g.database_id = d.id;

-- Ungranted rows keep workspace_id = NULL (legacy / unreachable): they are
-- filtered out of every workspace-scoped query. A destructive cleanup
-- (DELETE FROM databases WHERE workspace_id IS NULL) is intentionally NOT run.

-- The grant model is removed.
DROP TABLE workspace_database_grant;
```

- [ ] **Step 2: Touch the metastore lib so the embedded migrator re-embeds**

Add/adjust a trailing comment line in `crates/egret-metastore/src/lib.rs` (any no-op whitespace change). This is the known sqlx-migration-staleness gotcha: a new migration file does not recompile `egret-metastore` unless its source changes.

- [ ] **Step 3: Delete `grants.rs` and its module declaration**

Delete `crates/egret-database/src/grants.rs`. In `crates/egret-database/src/lib.rs` remove the line `mod grants;`. (`list_for_workspace` moves into `repo.rs` — Step 4.)

- [ ] **Step 4: Rework `DatabaseRepo` to be workspace-scoped**

In `crates/egret-database/src/repo.rs`:

Add `workspace_id` to the `Row` struct and `into_database` is unaffected (the DTO does not expose workspace_id). Change `create` to take a `workspace_id: Uuid` and bind it; add workspace-scoped `list_for_workspace`, `get_in_workspace`, `update_in_workspace`, `delete_in_workspace`; KEEP `get(id)` and `open_secret(id)` unscoped (used by the engine job after authorize); remove the `pool()` accessor if grants was its only user.

`create` signature + body:
```rust
/// Register a new database in `workspace_id`, sealing `secret` if provided.
pub async fn create(
    &self,
    workspace_id: Uuid,
    input: &DatabaseInput,
    created_by: Option<UserId>,
) -> Result<Database, DatabaseError> {
    let secret_id = match &input.secret {
        Some(s) if !s.is_empty() => Some(self.secrets.put("db_password", s).await?),
        _ => None,
    };
    let opts = serde_json::to_value(&input.options).unwrap_or(serde_json::json!({}));
    let row = sqlx::query_as::<_, Row>(
        "INSERT INTO databases (name, engine, host, port, options, secret_id, created_by, workspace_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
         RETURNING id, name, engine, host, port, options, secret_id, status",
    )
    .bind(&input.name)
    .bind(input.engine.as_str())
    .bind(&input.host)
    .bind(input.port)
    .bind(opts)
    .bind(secret_id)
    .bind(created_by.map(|u| u.0))
    .bind(workspace_id)
    .fetch_one(&self.db.pool)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(d) if d.is_unique_violation() => {
            DatabaseError::AlreadyExists(input.name.clone())
        }
        _ => DatabaseError::Db(e),
    });
    match row {
        Ok(r) => r.into_database(),
        Err(e) => {
            if let Some(sid) = secret_id {
                if let Err(del) = self.secrets.delete(sid).await {
                    tracing::warn!(secret_id = %sid, error = %del,
                        "failed to clean up orphaned secret after insert failure");
                }
            }
            Err(e)
        }
    }
}
```

> NOTE: `databases.name` has a global `UNIQUE` constraint today. Per-workspace, two workspaces may legitimately want the same connection name. This plan keeps the **global** unique constraint for now (simplest; matches the storage epic which also enforced per-workspace uniqueness but via a different table). If a collision is hit in testing it surfaces as `AlreadyExists` — acceptable for this epic. Do NOT change the constraint here.

`list_for_workspace` (moved from grants.rs, now ownership-based):
```rust
/// Databases owned by a workspace (ordered by name).
pub async fn list_for_workspace(&self, workspace_id: Uuid) -> Result<Vec<Database>, DatabaseError> {
    let rows = sqlx::query_as::<_, Row>(
        "SELECT id, name, engine, host, port, options, secret_id, status \
         FROM databases WHERE workspace_id = $1 ORDER BY name",
    )
    .bind(workspace_id)
    .fetch_all(&self.db.pool)
    .await?;
    rows.into_iter().map(Row::into_database).collect()
}
```

`get_in_workspace` (ownership-checked; the router uses this so a cross-workspace id is a 404):
```rust
/// Fetch a database that belongs to `workspace_id` (404 otherwise — IDOR-safe).
pub async fn get_in_workspace(&self, workspace_id: Uuid, id: Uuid) -> Result<Database, DatabaseError> {
    sqlx::query_as::<_, Row>(
        "SELECT id, name, engine, host, port, options, secret_id, status \
         FROM databases WHERE id = $1 AND workspace_id = $2",
    )
    .bind(id)
    .bind(workspace_id)
    .fetch_optional(&self.db.pool)
    .await?
    .ok_or_else(|| DatabaseError::NotFound(id.to_string()))?
    .into_database()
}
```

`update_in_workspace` / `delete_in_workspace`: copy the existing `update`/`delete` bodies but gate the initial existence check on `get_in_workspace(workspace_id, id)` (so an id from another workspace returns `NotFound` before any mutation), and add `AND workspace_id = $N` to the `UPDATE`/`DELETE`/`SELECT secret_id` statements. Keep the secret-rotation/cleanup logic identical. Keep `set_status(id, status)` unscoped (the router calls it only after `get_in_workspace`).

Replace the old unscoped `update`/`delete` with the `_in_workspace` variants (no caller needs the unscoped ones — the engine path only uses `get`/`open_secret`/`list_for_workspace`). Keep `list()` only if something still calls it; otherwise remove it (grep first).

- [ ] **Step 5: Write the failing repo test**

`crates/egret-database/tests/repo_workspace_scope.rs` (mirror the setup pattern of `crates/egret-catalog-engine/tests/storage_registry_api.rs` for `DATABASE_URL` gating + creating two workspaces). Tests:
```rust
// 1. create(ws_a, ..) then list_for_workspace(ws_a) returns it; list_for_workspace(ws_b) does not.
// 2. get_in_workspace(ws_a, id) ok; get_in_workspace(ws_b, id) -> NotFound.
// 3. update_in_workspace(ws_b, id, ..) -> NotFound (no mutation); update_in_workspace(ws_a, id, ..) ok.
// 4. delete_in_workspace(ws_b, id) -> NotFound; delete_in_workspace(ws_a, id) removes it.
```
Use a `secret: None` input so no master key is needed.

- [ ] **Step 6: Run the test (expect FAIL: methods don't exist yet), then implement Step 4, then re-run**

```
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-database --test repo_workspace_scope -- --test-threads=1
```
Expected after Step 4: PASS (or "skip: DATABASE_URL not set" if unset — it must be set).

- [ ] **Step 7: Workspace fmt + clippy + build**

```
cargo fmt -p egret-database -p egret-metastore -- --check
cargo clippy -p egret-database --all-targets
cargo build -p egret-database
```
All clean.

- [ ] **Step 8: Commit**

```
git add migrations/0031_databases_per_workspace.sql crates/egret-database crates/egret-metastore/src/lib.rs
git commit -m "feat(database): per-workspace schema + workspace-scoped repo; drop grants"
```

---

## Task 2: Workspace HTTP router + remove old routers + wiring

**Files:**
- Modify: `crates/egret-database/src/routes.rs` (replace admin/me routers with a workspace router)
- Modify: `crates/egret-database/src/lib.rs` (export `workspace_database_router`, drop `admin_database_router`/`me_database_router`)
- Modify: `crates/egret-server/src/lib.rs:139-146` (mount the new router)
- Test: `crates/egret-database/tests/workspace_routes_api.rs` (new; DB-gated)

- [ ] **Step 1: Replace the routers in `routes.rs`**

Remove `admin_database_router`, `me_database_router`, `register/list/detail/update/remove/test_saved/test_unsaved` (platform-gated versions), `grant/revoke`, `GrantBody`, `WsQuery`, `list_my_databases`, `is_platform_admin`, the `PERM_MANAGE` const. Add `use egret_workspace::context::WsContext;`. Add:

```rust
const PERM_MANAGE: &str = "workspace:manage_members";

pub fn workspace_database_router() -> Router<DatabaseState> {
    Router::new()
        .route("/ws/{workspace}/databases", post(create).get(list))
        .route("/ws/{workspace}/databases/test", post(test_unsaved))
        .route("/ws/{workspace}/databases/{id}", get(detail).put(update).delete(remove))
        .route("/ws/{workspace}/databases/{id}/test", post(test_saved))
}

async fn list(
    State(st): State<DatabaseState>,
    WsContext(ctx): WsContext,
) -> Result<Json<Vec<DatabaseDetailDto>>, DbApiError> {
    st.authz.require_workspace(&ctx, PERM_MANAGE).await?;
    let dbs = st.repo.list_for_workspace(ctx.workspace_id.0).await?;
    Ok(Json(dbs.into_iter().map(Into::into).collect()))
}

async fn create(
    State(st): State<DatabaseState>,
    WsContext(ctx): WsContext,
    Json(body): Json<DatabaseInput>,
) -> Result<Json<DatabaseDetailDto>, DbApiError> {
    st.authz.require_workspace(&ctx, PERM_MANAGE).await?;
    let db = st.repo
        .create(ctx.workspace_id.0, &body, Some(ctx.principal.user_id))
        .await?;
    Ok(Json(db.into()))
}

async fn detail(
    State(st): State<DatabaseState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> Result<Json<DatabaseDetailDto>, DbApiError> {
    st.authz.require_workspace(&ctx, PERM_MANAGE).await?;
    Ok(Json(st.repo.get_in_workspace(ctx.workspace_id.0, id).await?.into()))
}

async fn update(
    State(st): State<DatabaseState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Json(body): Json<DatabaseInput>,
) -> Result<Json<DatabaseDetailDto>, DbApiError> {
    st.authz.require_workspace(&ctx, PERM_MANAGE).await?;
    Ok(Json(
        st.repo
            .update_in_workspace(ctx.workspace_id.0, id, &body, Some(ctx.principal.user_id))
            .await?
            .into(),
    ))
}

async fn remove(
    State(st): State<DatabaseState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, DbApiError> {
    st.authz.require_workspace(&ctx, PERM_MANAGE).await?;
    st.repo.delete_in_workspace(ctx.workspace_id.0, id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn test_saved(
    State(st): State<DatabaseState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> Result<Json<TestOutcome>, DbApiError> {
    st.authz.require_workspace(&ctx, PERM_MANAGE).await?;
    let db = st.repo.get_in_workspace(ctx.workspace_id.0, id).await?; // 404 if not owned
    let secret = st.repo.open_secret(id).await?;
    let outcome = crate::tester::test_connection(
        db.engine, &db.host, db.port, &db.options, secret.as_deref(),
    ).await;
    let _ = st.repo.set_status(id, if outcome.ok { "ok" } else { "error" }).await;
    Ok(Json(outcome))
}

async fn test_unsaved(
    State(st): State<DatabaseState>,
    WsContext(ctx): WsContext,
    Json(body): Json<TestInput>,
) -> Result<Json<TestOutcome>, DbApiError> {
    st.authz.require_workspace(&ctx, PERM_MANAGE).await?;
    let outcome = crate::tester::test_connection(
        body.engine, &body.host, body.port, &body.options, body.secret.as_deref(),
    ).await;
    Ok(Json(outcome))
}
```

> `ctx.principal.user_id` is an `egret_core::UserId`; pass it as `Some(ctx.principal.user_id)` matching `create`/`update`'s `Option<UserId>`. Confirm the field path against `WorkspaceContext` (the engine uses `ctx.principal.user_id.0`). Keep `DatabaseDetailDto` and `TestInput`/`TestOutcome` exactly as they are.

- [ ] **Step 2: Update `lib.rs` exports**

In `crates/egret-database/src/lib.rs`:
```rust
pub use routes::{workspace_database_router, DatabaseState};
```
(remove `admin_database_router, me_database_router`).

- [ ] **Step 3: Rewire the mount in `egret-server/src/lib.rs`**

Replace lines 139-146 with:
```rust
if let Some(database) = state.database.clone() {
    let group = Router::new()
        .nest("/api", egret_database::workspace_database_router())
        .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
        .with_state(database);
    router = router.merge(group);
}
```

- [ ] **Step 4: Confirm the engine still compiles unchanged**

`egret-engine` calls `repo.list_for_workspace(ctx.workspace_id.0)` (now ownership) + `repo.get(id)` + `repo.open_secret(id)`. Step 4 of Task 1 KEEPS `get`/`open_secret` unscoped, so the engine needs no edit. Verify with `cargo build -p egret-engine`. If `get`/`open_secret` were accidentally removed, re-add them (unscoped).

- [ ] **Step 5: Write the failing route test**

`crates/egret-database/tests/workspace_routes_api.rs` (mirror `crates/egret-catalog-engine/tests/storage_registry_api.rs`: build the app via the same fixture, add a `ws_admin` member + a non-admin member, two workspaces). Assertions:
```
// POST /ws/{a}/databases (admin)        -> 200, body has the new id
// GET  /ws/{a}/databases                -> contains it; GET /ws/{b}/databases does not
// GET  /ws/{a}/databases/{id}           -> 200; GET /ws/{b}/databases/{id} -> 404 (IDOR)
// PUT  /ws/{b}/databases/{id}           -> 404 (no cross-ws mutation)
// DELETE /ws/{a}/databases/{id}         -> 200 ok
// non-admin member POST/GET             -> 403
// POST /ws/{a}/databases/test (bad host)-> 200 with { ok: false }  (tester returns outcome, not error)
```
Use `secret: None` (no master key needed). For the `test` endpoints, an unreachable host returns `TestOutcome { ok: false, .. }`.

- [ ] **Step 6: Run tests (FAIL → implement → PASS)**

```
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-database --test workspace_routes_api -- --test-threads=1
cargo build -p egret-engine -p egret-server
```

- [ ] **Step 7: fmt + clippy (workspace, since server/engine touched)**

```
cargo fmt --all -- --check
cargo clippy -p egret-database -p egret-engine -p egret-server --all-targets
```

- [ ] **Step 8: Commit**

```
git add crates/egret-database crates/egret-server
git commit -m "feat(database): /ws/{ws}/databases workspace router; remove admin+me routers and grants"
```

---

## Task 3: Frontend DatabasesPage + api + sidebar + SQL Lab picker

**Files:**
- Create: `frontend/src/api/databases.ts`
- Create: `frontend/src/routes/explore/DatabasesPage.tsx`
- Create: `frontend/src/routes/explore/__tests__/DatabasesPage.test.tsx`
- Modify: `frontend/src/components/layout/Sidebar.tsx` (add "Databases" to MANAGE)
- Modify: `frontend/src/api/sqllab.ts:32` (switch `/me/databases?workspace=` → `/ws/{ws}/databases`)
- Modify: the route table (wherever `StoragePage` is routed — add the `databases` route)
- Modify/remove: `frontend/src/routes/admin/Databases.tsx` placeholder reference if it is routed

- [ ] **Step 1: Write `api/databases.ts`** — mirror `frontend/src/api/storage.ts` exactly, swapping the resource:

```ts
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiJson } from "@/api/client";

export type Engine = "trino" | "starrocks";

export interface ConnOptions {
  user?: string | null;
  http_scheme?: string | null;
  catalog?: string | null;
  database?: string | null;
}

export interface DatabaseConn {
  id: string;
  name: string;
  engine: Engine;
  host: string;
  port: number;
  options: ConnOptions;
  has_secret: boolean;
  status: string;
}

export interface DatabaseConnInput {
  name: string;
  engine: Engine;
  host: string;
  port: number;
  options: ConnOptions;
  secret: string | null; // write-only; null/"" keeps existing on update
}

export interface TestOutcome { ok: boolean; message?: string | null; }

export const databaseKeys = {
  all: (ws: string) => ["databases", ws] as const,
};

export function useDatabases(ws: string) {
  return useQuery({
    queryKey: databaseKeys.all(ws),
    queryFn: () => apiJson<DatabaseConn[]>(`/ws/${ws}/databases`),
  });
}

export function useCreateDatabase(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (input: DatabaseConnInput) =>
      apiJson<DatabaseConn>(`/ws/${ws}/databases`, { method: "POST", body: JSON.stringify(input) }),
    onSuccess: () => qc.invalidateQueries({ queryKey: databaseKeys.all(ws) }),
  });
}

export function useUpdateDatabase(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, input }: { id: string; input: DatabaseConnInput }) =>
      apiJson<DatabaseConn>(`/ws/${ws}/databases/${id}`, { method: "PUT", body: JSON.stringify(input) }),
    onSuccess: () => qc.invalidateQueries({ queryKey: databaseKeys.all(ws) }),
  });
}

export function useDeleteDatabase(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => apiJson(`/ws/${ws}/databases/${id}`, { method: "DELETE" }),
    onSuccess: () => qc.invalidateQueries({ queryKey: databaseKeys.all(ws) }),
  });
}

export function useTestDatabase(ws: string) {
  return useMutation({
    mutationFn: (id: string) =>
      apiJson<TestOutcome>(`/ws/${ws}/databases/${id}/test`, { method: "POST" }),
  });
}
```
> Match the actual `apiJson` signature in `frontend/src/api/client.ts` (verify whether it takes `(path, init)` like above or wraps differently — copy the call shape `storage.ts` uses, since that is proven).

- [ ] **Step 2: Write `DatabasesPage.tsx`** — mirror `frontend/src/routes/explore/StoragePage.tsx` structure (registry table + Add/Edit `Dialog` + `ConfirmDeleteDialog` + 403 alert + loading skeleton), with these differences:
  - Columns: **Name · Engine · Host:Port · Status · Secret · Actions**. Status renders a `Badge` (`ok`→default, `error`→destructive, else secondary). Secret badge = `has_secret ? "Configured" : "Not set"` (same as storage).
  - Form fields: Name (Input); **Engine** shadcn `Select` (`trino` / `starrocks`); Host (Input); Port (Input, number); Options: User, Catalog, Schema (`database`), HTTP scheme — map to `options.{user,catalog,database,http_scheme}`; **Password** (Input, write-only → `secret`).
  - A **Test connection** button inside the row actions (or the edit dialog) that calls `useTestDatabase(ws)` and toasts/sets a small inline result (`ok` ✓ / `error` ✗ with message). Use the existing toast/inline pattern from the codebase; if none, show the outcome text next to the button.
  - No Browse/FileBrowser (DB has no object browser). Otherwise reuse the storage page's dialog/validation/secret-write-only conventions verbatim.

- [ ] **Step 3: Add the route + sidebar entry**
  - In `Sidebar.tsx`, add to the MANAGE `NavGroup` (after Storage): `{ label: "Databases" }` (the `slug(label)` helper maps it to `/explore/:ws/databases`). Pick an existing lucide icon (e.g. `Database`).
  - Register the route `/(/explore/:workspace/databases)` → `<DatabasesPage />` wherever `StoragePage`'s route is declared (grep for `StoragePage` in the router).

- [ ] **Step 4: Switch the SQL Lab picker source**

In `frontend/src/api/sqllab.ts:32`, change:
```ts
queryFn: () => apiJson<DatabaseListItem[]>(`/me/databases?workspace=${encodeURIComponent(ws)}`),
```
to:
```ts
queryFn: () => apiJson<DatabaseListItem[]>(`/ws/${encodeURIComponent(ws)}/databases`),
```
The workspace endpoint returns `DatabaseConn[]` whose `{ id, name, engine }` are a superset of `DatabaseListItem` — verify `DatabaseListItem`'s fields are present (id/name/engine) and that extra fields are ignored. Update `SqlLabTabs.test.tsx:21` mock URL from `/me/databases` to `/ws/<ws>/databases` so the existing SQL Lab test stays green.

- [ ] **Step 5: Remove the dead `/admin` Databases placeholder**

If `frontend/src/routes/admin/Databases.tsx` is routed in the admin router, remove that route + the file (DB management now lives in the workspace MANAGE zone). If it is only a placeholder not yet routed, leave it but do not link to it.

- [ ] **Step 6: Write `DatabasesPage.test.tsx`** — mirror `frontend/src/routes/explore/__tests__/StoragePage.test.tsx` (mock `@/api/databases`, render inside `MemoryRouter` + `QueryClientProvider`). Tests: list renders name/engine/host:port/status/secret badges; Add dialog → `useCreateDatabase` called with the engine/host/port/options/secret payload; Edit prefills + `useUpdateDatabase`; Delete confirm → `useDeleteDatabase`; Test button → `useTestDatabase`; 403 → admin notice; secret never rendered as plaintext.

- [ ] **Step 7: Run the full FE gate**

```
cd frontend && npm run lint && npm run typecheck && npm test && npm run build
```
All green (including the updated `SqlLabTabs.test.tsx`).

- [ ] **Step 8: Commit**

```
git add frontend/src
git commit -m "feat(web): per-workspace Databases page + api + sidebar; SQL Lab reads workspace databases"
```

---

## Self-review (controller, before final review)
- [ ] Migration backfills granted DBs, leaves orphans NULL, drops the grant table; metastore lib touched.
- [ ] Repo: `create` takes `workspace_id`; `list_for_workspace`/`get_in_workspace`/`update_in_workspace`/`delete_in_workspace` are workspace-scoped; `get`/`open_secret` stay unscoped for the engine job; `grants.rs` gone.
- [ ] Router gated `workspace:manage_members`; cross-ws id → 404 (IDOR); non-admin → 403; old admin/me routers + mounts removed.
- [ ] Engine unchanged and still compiles; `authorize()` is now ownership via `list_for_workspace`.
- [ ] FE: DatabasesPage in MANAGE; `api/databases.ts` paths have no `/api` prefix; SQL Lab picker + its test switched to `/ws/{ws}/databases`; admin placeholder not linked.
- [ ] **Before merge:** full local CI (cargo fmt --all --check, clippy, `cargo test --workspace`, FE lint/typecheck/test/build) + the manual Trino integration test + live SQL Lab smoke (pick a workspace DB, run a query) — GitHub Actions is out of quota.

## Out of scope
Named-secret registry + rewiring connection forms (Epic B); cross-workspace DB sharing; Ranger; GCS/Azure storage backends.
