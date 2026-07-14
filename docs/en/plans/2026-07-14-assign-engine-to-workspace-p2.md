# Assign Engine → Workspace — P2 (Surfaces) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Surface engine↔workspace assignments in the UI — assign from both Control-Plane sides, and let a workspace see its assigned (egret-provisioned) engines read-only, with the storage/secret each catalog maps to, alongside its self-service connections.

**Architecture:** P1 (merged, PR #247) shipped the backend assignment + materialized read-only delegated `query.databases` rows + assignment endpoints. P2 adds (a) two small backend reads — `engine_id` on the workspace database **list** DTO, and a ws-member-gated **engine view** (`GET /ws/{ws}/engines[/{id}]`) resolving catalogs→storage/secret (the deferred P1 T9); and (b) the frontend: CP Engine-detail **Workspaces** tab, CP Workspace-detail **Assigned engines** section, the workspace **Engines page two-group** read-only view, and a SQL-Lab **"egret"** tag.

**Tech Stack:** Rust (axum, sqlx, utoipa) — schemas `engine_mgmt.*`, `query.*`; React 18 + TS, TanStack Query, orval client, shadcn/ui, Vitest.

## Facts (verified)

- Generated hooks already exist (P1 regen): `useControlPlaneEnginesListEngineWorkspaces`, `useControlPlaneEnginesSetEngineWorkspaces` (CP engine→workspaces); `useControlPlaneEnginesListWorkspaceEngines`, `useControlPlaneEnginesAssignWorkspaceEngine`, `useControlPlaneEnginesUnassignWorkspaceEngine` (workspace-admin→engines).
- `Database` model (`frontend/src/api/generated/model/database.ts`) has `engine_id?: string | null`. But the **list** DTOs (`DatabaseListDto`, `DatabaseDetailDto`) do **not** yet expose `engine_id` — the ws Engines page reads `useDatabases(ws)` (→ `DatabaseDetailDto[]`). B1 fixes this.
- Workspace Engines page: `frontend/src/routes/explore/DatabasesPage.tsx` (uses `useDatabases(ws)`, `DatabaseConn = DatabaseDetailDto`, an `Add`/`Edit`/`Delete` per-row; already branches on `conn.delegated`).
- CP Engine detail: `frontend/src/routes/control-plane/EngineDetail.tsx` (tabbed).
- CP Workspace detail: `frontend/src/routes/control-plane/WorkspaceDetail.tsx` (from PR #246) — the `ResourcesTab({ workspaceId })` is where the Assigned-engines section goes.
- `workspace_database_router` (`egret-query`) uses `WsContext` (workspace-member auth) + holds a `Db` — the natural home for the ws-member engine view (B2).

---

## File structure

**Backend**
- Modify `crates/egret-query/src/registry/routes.rs` — add `engine_id` to `DatabaseDetailDto`/`DatabaseListDto` (B1); add ws engine-view routes (B2).
- Modify `crates/egret-query/src/registry/*` — a read that joins `engine_mgmt.engines`/`engine_catalogs` + resolves storage/secret names (B2).

**Frontend**
- `frontend/src/api/generated/*` — regenerated.
- `frontend/src/routes/control-plane/EngineDetail.tsx` — Workspaces tab (F1).
- `frontend/src/routes/control-plane/WorkspaceDetail.tsx` — Assigned-engines in `ResourcesTab` (F2).
- `frontend/src/routes/explore/DatabasesPage.tsx` — two-group split + read-only provisioned cards (F3).
- `frontend/src/routes/explore/**` SQL-Lab engine picker — "egret" tag (F4).
- `__tests__` for each.

---

## Task B1: expose `engine_id` on the workspace database DTOs

**Files:** Modify `crates/egret-query/src/registry/routes.rs`; Test: the crate's route tests

- [ ] **Step 1: Write the failing test**

```rust
#[sqlx::test]
async fn ws_database_list_exposes_engine_id(pool: PgPool) {
    let (app, cookie, ws) = ws_db_app(pool.clone()).await;     // existing harness
    let db = seed_provisioned_db(&pool, ws).await;             // engine_id set (P1 helper)
    let body = body_json(send(&app, &cookie, "GET", &format!("/api/ws/{ws}/databases"), None).await).await;
    let row = body.as_array().unwrap().iter().find(|d| d["id"] == db.to_string()).unwrap();
    assert!(row["engine_id"].is_string(), "list DTO carries engine_id");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p egret-query ws_database_list_exposes_engine_id -- --nocapture`
Expected: FAIL — `engine_id` absent from the list response.

- [ ] **Step 3: Add the field**

In `routes.rs`, add `pub engine_id: Option<String>` to the DTO struct(s) the list/detail handlers return (`DatabaseDetailDto` and/or `DatabaseListDto`), and populate it from `db.engine_id.map(|u| u.to_string())` in the `From<Database>`/mapping. Update the `#[utoipa::ToSchema]` derive accordingly.

> Confirm which DTO the `/ws/{ws}/databases` list handler actually returns (grep the list handler) and add `engine_id` there; also add it to the single-get detail DTO so the FE detail view is consistent.

- [ ] **Step 4: Run to verify it passes** — PASS; then `cargo test -p egret-query`.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-query/src/registry/routes.rs
git commit -m "feat(query): expose engine_id on workspace database DTOs"
```

---

## Task B2: ws-member engine view (catalogs → storage/secret)

**Files:** Modify `crates/egret-query/src/registry/routes.rs` (+ a store/query helper); Test

- [ ] **Step 1: Write the failing test**

```rust
#[sqlx::test]
async fn ws_engine_view_lists_assigned_engines_with_catalogs(pool: PgPool) {
    let (app, cookie, ws) = ws_db_app(pool.clone()).await;
    let engine = seed_assigned_engine_with_catalog(&pool, ws).await; // engine + engine_catalog + engine_workspaces
    // list
    let list = body_json(send(&app, &cookie, "GET", &format!("/api/ws/{ws}/engines"), None).await).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    // detail
    let d = body_json(send(&app, &cookie, "GET", &format!("/api/ws/{ws}/engines/{engine}"), None).await).await;
    assert_eq!(d["name"], "trino-prod");
    assert!(d["catalogs"].is_array());
    // an unassigned engine → 404
    let other = seed_unassigned_engine(&pool).await;
    assert_eq!(send(&app, &cookie, "GET", &format!("/api/ws/{ws}/engines/{other}"), None).await.status(),
               StatusCode::NOT_FOUND);
}
```

- [ ] **Step 2: Run to verify it fails** — 404 (routes unregistered).

- [ ] **Step 3: Implement**

Add to `workspace_database_router` (or a sibling router mounted under `/api/ws` on the same `WsContext`-capable state):
- `GET /ws/{workspace}/engines` → engines assigned to the workspace: `SELECT e.* FROM engine_mgmt.engines e JOIN engine_mgmt.engine_workspaces ew ON ew.engine_id=e.id WHERE ew.workspace_id=$ws`. Return `[{id,name,type,status}]`.
- `GET /ws/{workspace}/engines/{engine_id}` → only if assigned to `{workspace}` (else 404); return `{name,type,status,catalogs:[{name,kind,storage_name,secret_name}]}`. Catalogs from `engine_mgmt.engine_catalogs`; `storage_name` via the engine's `warehouse_id` → warehouse → storage-connection name; `secret_name` from `secret_ref` (name only, never the value).

Gate `catalog:read` via `WsContext` (the router already authenticates the workspace member). Use raw sqlx reads (this is a cross-seam read; a raw query in `egret-query` avoids a new crate dependency). utoipa-annotate + register.

> Confirm the warehouse→storage-connection resolution path (grep how `warehouses.storage_conn_id`/`storage.storage_connections` join to a name) and the `secret_ref` → secret-name table (`workspace_secrets`/`secrets`). If a resolution is heavy or ambiguous, return `storage_name: null`/`secret_name: null` for that catalog rather than failing the whole response.

- [ ] **Step 4: Run to verify it passes** — PASS.

- [ ] **Step 5: Commit**

```bash
git commit -am "feat(query): ws-member read-only view of assigned engines (catalogs, storage, secret)"
```

---

## Task B3: regenerate the client

- [ ] **Step 1: Emit + regen**

Run: `cargo run -p egret-bin --bin egret -- openapi > openapi.json && (cd frontend && npm run gen:api && npm run typecheck)`
Expected: new `/ws/{ws}/engines` paths + `engine_id` on the database DTOs appear; client regenerates; typecheck clean.

- [ ] **Step 2: Commit** — `git add openapi.json frontend/src/api/generated && git commit -m "chore(api): regenerate client for ws engine view + engine_id"`

---

## Task F1: CP Engine-detail — Workspaces tab

**Files:** Modify `frontend/src/routes/control-plane/EngineDetail.tsx`; Test: `__tests__/EngineDetail.test.tsx` (create if absent)

- [ ] **Step 1: Write the failing test** — mock `useControlPlaneEnginesListEngineWorkspaces` (returns `[wsA]`) + `useAdminWorkspacesListWs` (all workspaces); switch to the Workspaces tab; assert the assigned workspace is checked and toggling a row + Save calls `useControlPlaneEnginesSetEngineWorkspaces` with the new `workspaceIds`.

- [ ] **Step 2: Run to verify it fails** — module/tab not present.

- [ ] **Step 3: Implement**

Add a **Workspaces** tab to `EngineDetail.tsx`. Load all workspaces (`useAdminWorkspacesListWs`) + the engine's assigned set (`useControlPlaneEnginesListEngineWorkspaces(engineId)`). Render a `FilterableTable` with a checkbox per workspace (checked = assigned); a **Save** button calls `useControlPlaneEnginesSetEngineWorkspaces` with the full selected `workspaceIds` set; invalidate the list query on success. Show a hint that only JWT-capable engines can be assigned (the backend 400s otherwise — surface the error message as a toast).

> Match `EngineDetail.tsx`'s existing tab pattern (grep its `Tabs`/`TabsTrigger`). Confirm the response envelope shape (`.data?.status === 200 ? .data.data : []`).

- [ ] **Step 4: Verify PASS** — `npx vitest run src/routes/control-plane/__tests__/EngineDetail.test.tsx`.

- [ ] **Step 5: Commit** — `git commit -am "feat(control-plane): engine detail — assign to workspaces tab"`

---

## Task F2: CP Workspace-detail — Assigned engines section

**Files:** Modify `frontend/src/routes/control-plane/WorkspaceDetail.tsx`; Test: extend `__tests__/WorkspaceDetail.test.tsx`

- [ ] **Step 1: Write the failing test** — in the Resources tab, mock `useControlPlaneEnginesListWorkspaceEngines(wsId)` → `[{id,name,type,status}]`; assert the engine name renders; clicking **Remove** calls `useControlPlaneEnginesUnassignWorkspaceEngine`; the **Assign engine** dialog (picking an unassigned engine) calls `useControlPlaneEnginesAssignWorkspaceEngine`.

- [ ] **Step 2: Run to verify it fails.**

- [ ] **Step 3: Implement**

In `ResourcesTab({ workspaceId })`, add an **Assigned engines** section above the resource cards: list `useControlPlaneEnginesListWorkspaceEngines(workspaceId)` rows (name · type · status · Remove), plus an **Assign engine** dialog whose picker is `useControlPlaneEnginesListEngines()` minus already-assigned; assign/unassign via the generated mutations; invalidate the list on success; toast on the JWT-not-enabled 400.

- [ ] **Step 4: Verify PASS.**

- [ ] **Step 5: Commit** — `git commit -am "feat(control-plane): workspace detail — assign/remove engines"`

---

## Task F3: Workspace Engines page — two-group read-only view

**Files:** Modify `frontend/src/routes/explore/DatabasesPage.tsx`; Test: `__tests__/DatabasesPage.test.tsx`

- [ ] **Step 1: Write the failing test** — mock `useDatabases(ws)` returning one row with `engine_id` set and one without; assert two sections render — **Provisioned by egret** (the engine_id row, **no** Edit/Delete buttons) and **Your connections** (the other, with Edit/Delete). Mock `useWsEngineView` (the B2 hook) for the provisioned card's catalogs/storage/secret.

- [ ] **Step 2: Run to verify it fails.**

- [ ] **Step 3: Implement**

Partition `useDatabases(ws)` by `conn.engine_id`: 
- **Provisioned by egret** (engine_id set): read-only cards — name · engine type · status; expand to show catalogs (from the B2 `GET /ws/{ws}/engines/{engine_id}` view) with each catalog's `storage_name`/`secret_name`. **No** Add/Edit/Delete/Test.
- **Your connections** (engine_id null): the existing self-service list unchanged (Add/Edit/Delete/Test).

Keep the existing `conn.delegated` styling. Use the generated ws-engine-view hook (from B3) for the rich per-engine details, keyed by engine_id.

> Confirm `DatabaseConn`/`DatabaseDetailDto` now carries `engine_id` after B1+B3; if `useDatabases`'s `select` drops it, widen the mapping.

- [ ] **Step 4: Verify PASS.**

- [ ] **Step 5: Commit** — `git commit -am "feat(explore): workspace Engines page — provisioned (read-only) vs self-service groups"`

---

## Task F4: SQL Lab — tag provisioned engines

**Files:** Modify the SQL-Lab engine/database picker; Test

- [ ] **Step 1: Write the failing test** — the engine picker lists both a provisioned (engine_id set) and a self-service connection; the provisioned one shows an "egret" tag/badge.

- [ ] **Step 2: Run to verify it fails.**

- [ ] **Step 3: Implement**

In the SQL-Lab database picker (grep for where `useDatabases`/`useDatabasesList` feeds the picker), render an "egret" badge next to entries with `engine_id` set. No behavioural change — both are queryable `databases` rows; the badge is informational.

> Find the exact picker component (grep `useDatabases` under `src/routes/explore` / `src/components/sqllab`). Confirm the picker's item type carries `engine_id`.

- [ ] **Step 4: Verify PASS.**

- [ ] **Step 5: Commit** — `git commit -am "feat(sqllab): tag egret-provisioned engines in the database picker"`

---

## Task F5: full verification + PR

- [ ] **Step 1: Frontend CI** — `cd frontend && npm run lint && npm run typecheck && npx vitest run && npm run build` → all green.
- [ ] **Step 2: Backend CI** — `cargo fmt --all --check && cargo clippy -p egret-query --all-targets -- -D warnings && cargo test -p egret-query` → green.
- [ ] **Step 3: Live smoke (manual, not automated)** — assign a JWT engine to a workspace via CP; as a workspace member open `/explore/:ws/engines` → the provisioned engine shows read-only with its catalogs/storage/secret and no edit/delete; run a SQL-Lab query against it → rows (delegated). Manual Playwright-MCP driving.
- [ ] **Step 4: Finish** — `superpowers:finishing-a-development-branch`. PR title: `feat(engine): engine→workspace assignment surfaces (P2)`.

---

## Self-review notes

- **Spec coverage (P2):** CP Engine-detail Workspaces tab (F1), CP Workspace-detail assign (F2), workspace two-group Engines page (F3), SQL Lab tag (F4); the deferred P1 T9 rich read-only view ships here as B2; B1 unblocks the FE partition.
- **Auth:** F1/F2 endpoints are platform-admin (P1). B2's ws view is ws-member (`WsContext`, `catalog:read`) — the only new auth surface, placed in the query seam that already does `WsContext`.
- **No new query/auth mechanics:** provisioned rows remain plain `delegated` `databases` rows; P2 is display + assignment UX only.
- **Verify-before-invent flags:** which DTO the ws list handler returns (B1), the warehouse→storage + secret_ref→name resolution (B2), `EngineDetail`'s tab pattern (F1), `DatabaseConn` carrying `engine_id` post-regen (F3), and the SQL-Lab picker component (F4) — each called out inline.
