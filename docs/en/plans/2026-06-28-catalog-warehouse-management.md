# Catalog — warehouse management Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the single auto-provisioned warehouse per workspace with N user-managed warehouses, each on a registered storage connection; the Catalog area becomes warehouse list → detail; catalog tokens become per-warehouse.

**Architecture:** A new `workspace_warehouses` table (1:N, replacing the 1:1 `workspace_warehouse`) maps each managed warehouse to a lakekeeper warehouse uuid + an optional registry storage connection. `WarehouseStore` does CRUD (create → lakekeeper `create_warehouse`, delete → `delete_warehouse`). Browse + token routes are rescoped under `/warehouses/{wid}`. `front_auth` resolves the warehouse from the token (per-warehouse). The lazy auto-provision and the legacy single-storage table are removed.

**Tech Stack:** Rust (axum, sqlx 0.8, Postgres, embedded lakekeeper), egret-catalog / egret-catalog-engine / egret-server; React 18 + Vite + TanStack Query + shadcn; Vitest.

**Spec:** `docs/en/specs/2026-06-28-catalog-warehouse-management-design.md`. **Branch:** `feat/catalog-warehouses` (off main).

**Key facts (verified):**
- Migration head `migrations/0033_databases_name_per_workspace.sql` → new file `0034_workspace_warehouses.sql`.
- `workspace_warehouse` (migration 0025): `(workspace_id PK, warehouse_id)` 1:1. `CatalogStore` methods in `crates/egret-catalog/src/service/warehouse.rs`: `upsert_warehouse`, `warehouse_for_workspace`, `warehouse_for_slug`.
- **Dev data:** 1 `workspace_warehouse` row (eng), **0 `workspace_storage` (singular) rows** (the eng warehouse was provisioned from config `default_storage`, NOT a DB row), 3 `catalog_tokens` (eng), 1 `workspace_storages` registry row (minio-main). → the backfill CANNOT synthesize a connection from a legacy storage row; existing warehouses are backfilled with `storage_conn_id = NULL` (= "config default storage"), and the storage-resolution path falls back to `default_storage` when `storage_conn_id` is NULL.
- `catalog_tokens` table: `(id, workspace_id, token_hash, label, created_at, revoked_at)` + `permissions` + `namespace_scope` (added by later migrations). Token store: `crates/egret-catalog/src/service/tokens.rs` (`CatalogToken`, `TokenAuth { workspace_id, permissions, namespace_scope }`, `create_token(workspace_id, …)`, `authenticate_token`).
- `crates/egret-catalog-engine/src/provision_ws.rs`: `build_storage_profile(cfg: &StorageConfig, slug) -> StorageProfile` (key_prefix = `workspaces/{slug}/warehouse`); `provision_workspace_warehouse(...)` (calls lakekeeper `create_warehouse`, returns the warehouse uuid). lakekeeper `delete_warehouse`/`list_warehouses` are reachable the same way (`<EngineApiServer as WarehouseService<_,_,_>>::…`).
- `browse.rs`: `ensure_warehouse` (lazy auto-provision, lines ~423-490, calls `provision_workspace_warehouse`), `conn_s3` (resolves a registry connection's `StorageConfig` via the 2-hop named secret — REUSE this to build a warehouse's StorageProfile), catalog routes `summary`/`list_namespaces`/tables/table-detail/tokens, legacy `get_storage`/`update_storage` (`/catalog/storage`). `BrowseState` holds `store: CatalogStore`, `engine` (lakekeeper ApiContext), `secret_store`, `workspace_secrets`, `default_storage: StorageConfig`, `authz`, `system_user`, `workspaces`.
- `crates/egret-catalog-engine/src/front_auth.rs`: iceberg REST auth — `authenticate_token` → `TokenAuth`; resolves the warehouse via `warehouse_for_workspace`. `crates/egret-catalog-engine/src/provisioner.rs`: the workspace-create auto-provision hook.
- FE: `frontend/src/api/catalog.ts` (hooks `useCatalogSummary`/`useCatalogNamespaces(ws)`/`useNamespaceContents(ws,ns)`/`useTableDetail(ws,ns,table)`/`useCatalogTokens(ws)`/`useMintToken`/`useRevokeToken` — all workspace-level, paths `/ws/${ws}/catalog/…`). `frontend/src/routes/explore/catalog/` (`CatalogArea.tsx` with nested `CatalogBrowse`/`CatalogAccess`). `App.tsx`: `<Route path="catalog" element={<CatalogArea/>}>` with nested routes. `useStorageConns(ws)` lists registry connections.
- Test DB `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`; MinIO + Trino running; `--test-threads=1`; touch `crates/egret-metastore/src/lib.rs` after a migration. The catalog integration tests live under `crates/egret-catalog-engine/tests/` (mirror `catalog_provision_storage_api.rs` / `catalog_tokens_api.rs`).

---

## Task 1: Migration + `WarehouseStore` + token `warehouse_id`

**Files:**
- Create: `migrations/0034_workspace_warehouses.sql`
- Modify: `crates/egret-catalog/src/service/warehouse.rs` (WarehouseStore CRUD)
- Modify: `crates/egret-catalog/src/service/tokens.rs` (warehouse_id on create/list/auth)
- Modify: `crates/egret-metastore/src/lib.rs` (re-embed touch)
- Test: `crates/egret-catalog/tests/warehouse_store.rs` (DB-gated) — or extend an existing catalog store test

- [ ] **Step 1: Migration `migrations/0034_workspace_warehouses.sql`**
```sql
-- N managed warehouses per workspace, each (optionally) on a registry storage
-- connection. storage_conn_id is NULL for warehouses backfilled from the old
-- auto-provision (their storage is the config `default_storage`); new warehouses
-- always set it.
CREATE TABLE workspace_warehouses (
    id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id    uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    warehouse_id    uuid NOT NULL,                       -- lakekeeper warehouse uuid
    name            text NOT NULL,
    storage_conn_id uuid REFERENCES workspace_storages(id),
    created_at      timestamptz NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, name)
);
CREATE INDEX workspace_warehouses_workspace_id_idx ON workspace_warehouses(workspace_id);

-- Per-warehouse tokens.
ALTER TABLE catalog_tokens ADD COLUMN warehouse_id uuid REFERENCES workspace_warehouses(id) ON DELETE CASCADE;

-- Backfill the existing 1:1 warehouses as managed "default" warehouses (NULL conn).
INSERT INTO workspace_warehouses (workspace_id, warehouse_id, name, storage_conn_id)
SELECT workspace_id, warehouse_id, 'default', NULL FROM workspace_warehouse;

-- Point existing tokens at their workspace's default warehouse.
UPDATE catalog_tokens t
SET warehouse_id = wh.id
FROM workspace_warehouses wh
WHERE wh.workspace_id = t.workspace_id AND wh.name = 'default';

-- Retire the legacy tables (workspace_storage singular is empty in practice).
DROP TABLE workspace_warehouse;
DROP TABLE workspace_storage;
```
> `catalog_tokens.warehouse_id` is left NULLable (a pre-existing token with no warehouse is unusual but tolerated; the route layer treats NULL as the workspace's default). Verify there are no `workspace_warehouse` rows whose workspace lacks a row before relying on the token backfill (dev has exactly one, eng).

- [ ] **Step 2: Touch `crates/egret-metastore/src/lib.rs`** (no-op) to re-embed.

- [ ] **Step 3: `WarehouseStore` in `warehouse.rs`** — replace the 1:1 helpers with per-warehouse CRUD. Add a `WorkspaceWarehouse { id, warehouse_id, name, storage_conn_id: Option<Uuid>, created_at }` struct. Methods on `CatalogStore` (workspace-scoped; cross-ws id → `CatalogError::NotFound`):
```rust
// insert_warehouse(workspace_id, warehouse_id, name, storage_conn_id: Option<Uuid>) -> WorkspaceWarehouse
//   (INSERT ... RETURNING; unique(workspace_id,name) violation -> AlreadyExists)
// list_warehouses(workspace_id) -> Vec<WorkspaceWarehouse> (ORDER BY name)
// get_warehouse(workspace_id, id) -> WorkspaceWarehouse (404)
// rename_warehouse(workspace_id, id, name)
// delete_warehouse_row(workspace_id, id) -> returns the lakekeeper warehouse_id that was removed (so the route can call lakekeeper delete)
```
Keep a `lakekeeper_id(workspace_id, id) -> Uuid` helper (resolve `{wid}` → the lakekeeper `warehouse_id`, ownership-checked) for the browse/token routes. Keep `warehouse_for_slug` ONLY if still needed; front_auth will switch to a token-based resolution (Task 2), so `warehouse_for_workspace`/`upsert_warehouse` can be removed once callers are migrated — leave them until Task 2 compiles, then delete.

- [ ] **Step 4: Token store `warehouse_id`** (`tokens.rs`): add `warehouse_id: Uuid` to `CatalogToken`, `TokenAuth`, and the sqlx row; `create_token(workspace_id, warehouse_id, permissions, namespace_scope, …)` binds it; `list_tokens` becomes `list_tokens_for_warehouse(workspace_id, warehouse_id)` (`WHERE workspace_id=$1 AND warehouse_id=$2`); `authenticate_token` SELECTs `warehouse_id` into `TokenAuth`. Update the INSERT/SELECT column lists.

- [ ] **Step 5: Test** `crates/egret-catalog/tests/warehouse_store.rs` (DB-gated; mirror an existing catalog store test's workspace seeding): `insert_warehouse` then `list_warehouses`/`get_warehouse` (cross-ws → NotFound); `rename_warehouse`; `delete_warehouse_row` returns the lakekeeper id + removes the row; token `create_token(ws, wid, …)` → `list_tokens_for_warehouse(ws, wid)` returns it, a different wid does not; `authenticate_token` returns the warehouse_id.

- [ ] **Step 6: Run + checks**
```
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-catalog -- --test-threads=1
cargo fmt -p egret-catalog -p egret-metastore -- --check
cargo clippy -p egret-catalog --all-targets
cargo build -p egret-catalog
```
(The `egret-catalog-engine`/`egret-server` build will break until Task 2 migrates the callers — that's expected; keep `warehouse_for_workspace`/`upsert_warehouse` until Task 2 if needed to keep this crate's own build green.)

- [ ] **Step 7: Commit** `feat(catalog): workspace_warehouses model + per-warehouse tokens + store`. Stage `migrations/`, `crates/egret-catalog`, `crates/egret-metastore/src/lib.rs`.

---

## Task 2: Warehouse routes + lakekeeper lifecycle + rescope browse/tokens + front_auth + remove auto-provision

**Files:**
- Modify: `crates/egret-catalog-engine/src/browse.rs` (warehouse routes, rescope browse/tokens, remove ensure_warehouse + legacy storage routes)
- Modify: `crates/egret-catalog-engine/src/provision_ws.rs` (expose a `create_warehouse_on(cfg, name) -> Uuid` + `delete_warehouse(id)` reusable by the routes)
- Modify: `crates/egret-catalog-engine/src/front_auth.rs` (resolve warehouse from the token)
- Modify: `crates/egret-catalog-engine/src/provisioner.rs` + its caller (remove workspace-create auto-provision)
- Modify: `crates/egret-server/src/lib.rs` / `crates/egret-bin/src/main.rs` if state/route wiring changes
- Test: `crates/egret-catalog-engine/tests/warehouse_api.rs` (new) + update `catalog_tokens_api.rs` / `catalog_provision_storage_api.rs`

- [ ] **Step 1: Reusable lakekeeper helpers** in `provision_ws.rs`: factor `create_warehouse_on(engine, request_metadata, project_id, name: &str, cfg: &StorageConfig) -> Uuid` (builds the StorageProfile via `build_storage_profile(cfg, <key-prefix-seed>)` — use the warehouse name in the key_prefix so two warehouses don't collide, e.g. `warehouses/{warehouse_name}`; keep the existing behaviour for the default) and `delete_warehouse(engine, request_metadata, project_id, warehouse_id: Uuid)` (calls `<EngineApiServer as WarehouseService<_,_,_>>::delete_warehouse`). `provision_workspace_warehouse` can call `create_warehouse_on` internally.

- [ ] **Step 2: Warehouse CRUD routes** in `browse.rs` (gated `require_workspace_admin`; mirror the storages routes shape):
  - `GET /ws/{ws}/catalog/warehouses` → `store.list_warehouses(ws)` → JSON `[{id,name,storage_conn_id,storage_conn_name?,created_at}]` (join the storage connection name when set).
  - `POST /ws/{ws}/catalog/warehouses` body `{ name, storage_conn_id }` → resolve the `StorageConfig` from the connection (REUSE the `conn_s3` resolution: `store.get_storage_conn(ws, storage_conn_id)` + `workspace_secrets.resolve_plaintext` → a `StorageConfig`) → `create_warehouse_on(... name, &cfg)` → `store.insert_warehouse(ws, lk_id, name, Some(storage_conn_id))` → 201. A lakekeeper error (bad S3) → 400 with the trimmed message. `storage_conn_id` is REQUIRED for new warehouses.
  - `GET /ws/{ws}/catalog/warehouses/{wid}` → detail (name, storage connection, the lakekeeper location from `get_warehouse` management API if cheaply available, else just the stored fields).
  - `PUT /ws/{ws}/catalog/warehouses/{wid}` body `{ name }` → `rename_warehouse`.
  - `DELETE /ws/{ws}/catalog/warehouses/{wid}` → `delete_warehouse_row(ws, wid)` (returns lk id) → `provision_ws::delete_warehouse(lk_id)` → 204. (Order: remove the row only after lakekeeper delete succeeds, or vice-versa with a clear comment; prefer lakekeeper-first so a failed lakekeeper delete doesn't orphan the row.)

- [ ] **Step 3: Rescope browse routes** under `{wid}` (resolve `{wid}` → lakekeeper id via `store.lakekeeper_id(ws, wid)`, ownership-checked → 404 cross-ws):
  - `GET /ws/{ws}/catalog/warehouses/{wid}/namespaces`
  - `GET /ws/{ws}/catalog/warehouses/{wid}/namespaces/{ns}/tables`
  - `GET /ws/{ws}/catalog/warehouses/{wid}/namespaces/{ns}/tables/{table}`
  Reuse the existing namespace/table handler bodies, swapping the warehouse-id source from `ensure_warehouse`/`warehouse_for_workspace` to the resolved `{wid}`. **Remove** the old workspace-level `…/catalog/namespaces…` routes and the `summary` route (the FE now lists warehouses).

- [ ] **Step 4: Rescope token routes** under `{wid}`:
  - `GET /ws/{ws}/catalog/warehouses/{wid}/tokens` → `list_tokens_for_warehouse(ws, wid)`.
  - `POST /ws/{ws}/catalog/warehouses/{wid}/tokens` → `create_token(ws, wid, permissions, namespace_scope, …)`.
  - `DELETE /ws/{ws}/catalog/warehouses/{wid}/tokens/{id}` → revoke (scoped to ws+wid).
  Remove the old `…/catalog/tokens…` routes.

- [ ] **Step 5: `front_auth.rs` — resolve the warehouse from the token.** `authenticate_token` now yields `TokenAuth { workspace_id, warehouse_id, permissions, namespace_scope }`. Replace the `warehouse_for_workspace(workspace_id)` resolution with `store.lakekeeper_id(workspace_id, token.warehouse_id)` (the warehouse the token is bound to). The cross-tenant guard becomes: the request's warehouse prefix must equal the token's warehouse. Update the doc comment.

- [ ] **Step 6: Remove auto-provision + legacy storage.** Delete `ensure_warehouse` (and its calls — the rescoped routes now take an explicit `{wid}`). Remove the workspace-create auto-provision in `provisioner.rs` + its caller (the workspace-create handler no longer provisions a warehouse). Remove the legacy `GET/PUT /ws/{ws}/catalog/storage` routes + their handlers. Keep `default_storage` on `BrowseState` ONLY if a NULL-`storage_conn_id` warehouse still needs it for resolution; the rescoped browse uses the lakekeeper warehouse id directly (no storage resolution needed for browse — lakekeeper already knows the location), so `default_storage` may become unused → remove if so (grep first).

- [ ] **Step 7: Tests.** `crates/egret-catalog-engine/tests/warehouse_api.rs` (mirror `catalog_provision_storage_api.rs` fixture; real MinIO): create a warehouse on the `minio-main`-style registered connection → 201 + a real lakekeeper warehouse (browse its namespaces → 200, initially empty); cross-ws `{wid}` → 404; non-admin → 403; create with an unreachable storage connection → 400; delete → 204 + the lakekeeper warehouse is gone (a follow-up browse → 404/empty). Update `catalog_tokens_api.rs` to mint/list/revoke under `/warehouses/{wid}/tokens` and assert `authenticate_token` carries the warehouse. `DATABASE_URL=… cargo test -p egret-catalog-engine -- --test-threads=1` (retry once on a MinIO flake — do NOT weaken); `cargo build --workspace`; `cargo fmt --all -- --check`; `cargo clippy -p egret-catalog-engine -p egret-server --all-targets`.

- [ ] **Step 8: Commit** `feat(catalog): per-warehouse routes + lakekeeper lifecycle; token-scoped front_auth; remove auto-provision`.

---

## Task 3: Frontend — warehouse list → detail

**Files:**
- Modify: `frontend/src/api/catalog.ts` (warehouse hooks + rescope browse/token hooks to `wid`)
- Create: `frontend/src/routes/explore/catalog/WarehousesList.tsx` + `WarehouseDetail.tsx`
- Modify: `frontend/src/routes/explore/catalog/CatalogArea.tsx` + `CatalogBrowse.tsx` + `CatalogAccess.tsx` (consume `wid`)
- Modify: `frontend/src/App.tsx` (route nesting `catalog` → `catalog/:wid` with tabs)
- Test: `__tests__/WarehousesList.test.tsx` + `WarehouseDetail.test.tsx` + update existing catalog tests

- [ ] **Step 1: `api/catalog.ts`** — add `Warehouse { id, name, storage_conn_id, storage_conn_name, created_at }`; hooks `useWarehouses(ws)` (GET `/ws/${ws}/catalog/warehouses`), `useCreateWarehouse(ws)` (POST `{name, storage_conn_id}`), `useRenameWarehouse(ws)`, `useDeleteWarehouse(ws)`. **Rescope** `useCatalogNamespaces`/`useNamespaceContents`/`useTableDetail`/`useCatalogTokens`/`useMintToken`/`useRevokeToken` to take a `wid` and hit `/ws/${ws}/catalog/warehouses/${wid}/…`. Update `catalogKeys` to include `wid`. Drop `useCatalogSummary`.

- [ ] **Step 2: `WarehousesList.tsx`** (mirror `StoragePage.tsx`): table **Name · Storage · Created · Actions** (Open → navigate to `:wid`; Rename; Delete via ConfirmDeleteDialog) + **Add warehouse** dialog (name + a `Select` of `useStorageConns(ws)` connections). Empty state: "No warehouses yet — create one on a storage connection." 403 → admin notice.

- [ ] **Step 3: `WarehouseDetail.tsx`** (route `/explore/:workspace/catalog/:wid`): a header (warehouse name + storage connection) + tabs **Browse** / **Access** / **Details**, rendering the existing `CatalogBrowse`/`CatalogAccess` (now passed `wid`) + a Details panel (storage connection, rename). Mirror the lakekeeper `warehouse/[id]` tab layout (it's fine to use the app's existing Tabs primitive, not Vuetify).

- [ ] **Step 4: Wire routes** in `App.tsx`: `catalog` index → `<WarehousesList/>`; `catalog/:wid` → `<WarehouseDetail/>` (with its own Browse/Access/Details tabs). Update `CatalogArea` if it remains the layout shell, or replace it.

- [ ] **Step 5: Update `CatalogBrowse.tsx`/`CatalogAccess.tsx`** to read `wid` (from route params or props) and call the rescoped hooks.

- [ ] **Step 6: Tests** — `WarehousesList.test.tsx` (list/create-picks-a-storage/rename/delete/empty/403), `WarehouseDetail.test.tsx` (renders tabs; Browse/Access call hooks with the `wid`); update `CatalogBrowse.test.tsx`/`CatalogAccess.test.tsx`/`CatalogArea.test.tsx` for the `wid` rescope. `cd frontend && npm run lint && npm run typecheck && npm test && npm run build` — all green.

- [ ] **Step 7: Commit** `feat(web): catalog warehouse list → detail; per-warehouse browse/tokens`.

---

## Self-review (controller, before final review)
- [ ] Migration: `workspace_warehouses` (NULLable storage_conn_id) + `catalog_tokens.warehouse_id`; existing warehouse backfilled as `default` (NULL conn); tokens repointed; `workspace_warehouse` + `workspace_storage` dropped; metastore touched.
- [ ] `WarehouseStore` CRUD workspace-scoped (cross-ws → NotFound); `lakekeeper_id(ws, wid)` ownership-checked; tokens carry `warehouse_id`.
- [ ] Routes: warehouse CRUD (create→lakekeeper on a registry connection, delete→lakekeeper) gated admin; browse + tokens rescoped under `{wid}` (IDOR 404); old workspace-level catalog routes removed.
- [ ] `front_auth` resolves the warehouse from the token; cross-tenant guard intact.
- [ ] Auto-provision (`ensure_warehouse` + workspace-create) and legacy `/catalog/storage` removed; nothing still calls `warehouse_for_workspace`/`upsert_warehouse`.
- [ ] FE: warehouse list → detail (Browse/Access/Details), hooks rescoped to `wid`, Add-warehouse picks a storage connection.
- [ ] **Before merge:** full local CI (fmt/clippy/`cargo test --workspace`, FE lint/typecheck/test/build) + the manual Trino+MinIO catalog check + live smoke (create a warehouse on minio-main, browse namespaces, mint a token, delete the warehouse) — GitHub Actions out of quota.

## Out of scope (later sub-epics)
Namespace create/drop; table/view detail parity; tasks + statistics tabs; permissions tab; cross-workspace sharing; Ranger.
