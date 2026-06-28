# Catalog — warehouse management (per-workspace, N warehouses) — Design

**Date:** 2026-06-28 · **Status:** Design — approved · **Branch:** `feat/catalog-warehouses` (off main)
**Program:** Sub-epic 1 of the Catalog-console-parity program (warehouse management → namespace mgmt → table/view detail → tasks/statistics → permissions). Reference: the lakekeeper console (`data-portal/console`).

## Goal
Replace the single auto-provisioned warehouse per workspace with **N user-managed warehouses per workspace**, each created on a **registered storage connection** (the `workspace_storages` registry). The Catalog area becomes a warehouse **list → detail** experience (mirroring the lakekeeper console). Retire the lazy auto-provision and the legacy single-storage table. Catalog tokens become **per-warehouse**.

## Background (current state)
- `workspace_warehouse` (migration `0025`) maps a workspace **1:1** to one lakekeeper warehouse uuid. `CatalogStore::warehouse_for_workspace(ws)` returns that single id.
- The warehouse is lazily **auto-provisioned** (`browse.rs::ensure_warehouse` → `provision_workspace_warehouse` → lakekeeper `create_warehouse`) from the legacy single `workspace_storage` / config `default_storage`. Workspace-create also provisions one (`provisioner.rs`).
- Browse routes operate on that single warehouse: `GET /ws/{ws}/catalog` (summary), `…/catalog/namespaces`, `…/namespaces/{ns}/tables`, `…/namespaces/{ns}/tables/{table}`. The legacy single-storage editor is `GET/PUT /ws/{ws}/catalog/storage`.
- Catalog tokens (`egret-catalog/src/service/tokens.rs`) are scoped per **workspace_id** + `namespace_scope` (NOT per-warehouse).
- lakekeeper warehouse management is already wired: `create_warehouse`, `list_warehouses`, `delete_warehouse`, `update_storage` (used by `provision_ws.rs`/`provisioner.rs`).
- The Storage registry (`workspace_storages`, plural) + named secrets exist (prior epics); a registered connection resolves its credentials via the 2-hop named-secret path.
- FE: `explore/catalog/CatalogArea.tsx` (Browse + Access tabs) over the single warehouse.

## 1. Data model
Evolve the 1:1 map into a per-warehouse registry:
```sql
-- workspace_warehouses: N managed warehouses per workspace, each on a registry storage connection.
CREATE TABLE workspace_warehouses (
    id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id    uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    warehouse_id    uuid NOT NULL,                 -- the lakekeeper warehouse uuid
    name            text NOT NULL,
    storage_conn_id uuid NOT NULL REFERENCES workspace_storages(id),
    created_at      timestamptz NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, name)
);
CREATE INDEX workspace_warehouses_workspace_id_idx ON workspace_warehouses(workspace_id);
```
`catalog_token` gains a `warehouse_id uuid REFERENCES workspace_warehouses(id) ON DELETE CASCADE` — every token is now bound to one managed warehouse (plus the existing `namespace_scope`).

## 2. Backend (`egret-catalog` store + `egret-catalog-engine/browse.rs`)
- **`WarehouseStore`** (egret-catalog): `create(ws, name, storage_conn_id) -> WorkspaceWarehouse`, `list(ws)`, `get(ws, id)`, `rename(ws, id, name)`, `delete(ws, id)` — all workspace-scoped (cross-ws id → NotFound). `warehouse_for_workspace` is replaced by `get(ws, id)` / `list(ws)`; a `lakekeeper_id_for(ws, id)` resolves the `{wid}` → the lakekeeper warehouse uuid (ownership-checked).
- **Warehouse routes** (gated `workspace:manage_members`):
  - `GET /ws/{ws}/catalog/warehouses` — list managed warehouses (name, storage connection name, created).
  - `POST /ws/{ws}/catalog/warehouses` body `{ name, storage_conn_id }` → resolve the storage connection (build an S3 `StorageProfile` from its endpoint/region/bucket/key_prefix/access_key + 2-hop named secret) → lakekeeper `create_warehouse` (validates S3 access) → insert the mapping row → 201. A bad storage connection surfaces as a 400 with lakekeeper's reason.
  - `GET /ws/{ws}/catalog/warehouses/{wid}` — detail (name, storage connection, lakekeeper location).
  - `PUT /ws/{ws}/catalog/warehouses/{wid}` — rename.
  - `DELETE /ws/{ws}/catalog/warehouses/{wid}` → lakekeeper `delete_warehouse` + remove the row → 204.
- **Browse routes rescoped under a warehouse** (resolve `{wid}` → lakekeeper id, ownership-checked):
  - `GET /ws/{ws}/catalog/warehouses/{wid}/namespaces`
  - `GET /ws/{ws}/catalog/warehouses/{wid}/namespaces/{ns}/tables`
  - `GET /ws/{ws}/catalog/warehouses/{wid}/namespaces/{ns}/tables/{table}`
  (The old workspace-level `…/catalog/namespaces…` routes are removed; the summary route is replaced by the warehouse list + detail.)
- **Token routes rescoped under a warehouse:** `GET/POST /ws/{ws}/catalog/warehouses/{wid}/tokens`, `DELETE …/tokens/{id}`. The token store gains `warehouse_id`; mint/list/revoke filter by `(workspace_id, warehouse_id)`. `namespace_scope` is unchanged.
- **Remove** `ensure_warehouse` (lazy auto-provision) and the workspace-create auto-provision in `provisioner.rs`. New workspaces start with **no** warehouse; an admin creates one explicitly. `provision_ws.rs`'s storage-profile builder is reused by `WarehouseStore::create`.
- **Remove** the legacy single-storage routes `GET/PUT /ws/{ws}/catalog/storage`.

## 3. Migration
A new migration (`0034_workspace_warehouses.sql`) — backfill the existing auto-provisioned warehouses into the managed model, then retire the legacy pieces, in one transaction:
1. Create `workspace_warehouses` (+ index) and add `catalog_token.warehouse_id` (nullable first).
2. For each `workspace_warehouse (workspace_id, warehouse_id)` row: create a `workspace_storages` connection mirroring that workspace's legacy `workspace_storage` row (or the global `default_storage` when none) — name `catalog-default`, its secret wrapped as a named secret (reuse the named-secret backfill pattern); insert a `workspace_warehouses` row `(workspace_id, warehouse_id, name='default', storage_conn_id=<new conn>)`. The lakekeeper warehouse itself is untouched (it already exists on that S3 location).
3. Backfill `catalog_token.warehouse_id` = that workspace's new default `workspace_warehouses.id`; then make it `NOT NULL` + add the FK.
4. Drop the old `workspace_warehouse` table and the legacy `workspace_storage` (singular) table.
5. Touch `crates/egret-metastore/src/lib.rs` to re-embed.
> The data-shape work (mirroring legacy storage → a registry connection + named secret) is intricate; if a pure-SQL backfill is too complex for the secret wrapping, do the connection+secret creation in a small **one-shot Rust migration step** run at startup after the SQL migration (idempotent), rather than raw SQL. The plan picks the concrete mechanism.

## 4. Frontend (`explore/catalog/`)
The Catalog area (MANAGE) becomes a warehouse **list → detail**:
- **Warehouses list** (`CatalogArea` / a new `WarehousesList`): table **Name · Storage · Created · Actions** (Open / Rename / Delete) + **Add warehouse** dialog (name + a `Select` of registered storage connections from `useStorageConns`). Empty state: "No warehouses yet — create one on a storage connection." 403 → admin notice.
- **Warehouse detail** (route `/explore/:ws/catalog/:wid`): a header (name + storage + lakekeeper location) + tabs **Browse** (the existing namespace/table browse, rescoped to `{wid}`) · **Access** (the existing token UI, rescoped to `{wid}`) · **Details** (storage connection + location, rename). Mirror the lakekeeper `warehouse/[id]` tab layout.
- `api/catalog.ts`: add `useWarehouses(ws)` / `useCreateWarehouse` / `useRenameWarehouse` / `useDeleteWarehouse`; rescope the existing namespace/table/token hooks to take a `wid` and hit `…/warehouses/{wid}/…`.

## 5. Testing
- **Backend (DB + MinIO):** warehouse CRUD scoped to a workspace — create on a registered storage connection provisions a real lakekeeper warehouse (validates S3); cross-workspace `{wid}` → 404 (IDOR); non-admin member → 403; delete removes the lakekeeper warehouse + the row; create with an unreachable storage → 400. Browse + tokens are scoped to `{wid}` (a `{wid}` from another workspace → 404). Migration test: an existing auto-provisioned warehouse becomes a managed row + a backfilled storage connection + the workspace's tokens get `warehouse_id`.
- **Frontend (Vitest, mock):** warehouses list / create (picks a storage connection) / rename / delete; warehouse detail renders the three tabs and the browse/token hooks are called with the `wid`; 403 notice; empty state.
- **Manual:** the real-Trino + MinIO catalog integration check (per the `egret-manual-trino-integration-test` note) before merge; full local CI; live smoke — create a warehouse on `minio-main`, browse its namespaces, mint a token, delete the warehouse.

## 6. Risks
- **`create_warehouse` probes S3 live** — a misconfigured storage connection fails with lakekeeper's error; surface it as a 400.
- **Migration atomicity** — wrap backfill in one transaction; the existing warehouse must never lose its mapping or its lakekeeper id.
- **Auto-provision removal** — new workspaces have no warehouse; the Catalog UI must clearly prompt to create one (empty state), and any code that assumed `warehouse_for_workspace` always returns `Some` must handle "no warehouse selected".
- **`{wid}` → lakekeeper-id resolution must verify ownership** every time (cross-tenant IDOR).
- **CI** — GitHub Actions out of quota; run full local CI + the manual Trino check before admin-merge.

## 7. Out of scope (later sub-epics)
Namespace create/drop; table/view/generic-table detail parity (schema/snapshots/partition/properties); tasks + statistics tabs; permissions tab; cross-workspace warehouse sharing; Ranger.
