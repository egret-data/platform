# Workspace IA restructure — Catalog area + editable Storage (Phase A+B) — Design

**Date:** 2026-06-27
**Status:** Design — approved
**Branch:** `feat/workspace-admin-catalog` (continues; BE catalog-token routes already landed here)
**Parent:** architecture ADR-19 + ADR-19a (three-layer management; sidebar zones; Catalog area; editable Storage)

## 0. Goal

Restructure the per-workspace UI per ADR-19a:
- **Sidebar zones:** split the flat workspace nav into **EXPLORE** (SQL Lab, Datasets, Charts, Dashboards, Notebooks, Catalog) and **MANAGE** (Storage; later Members/Connections).
- **Catalog is a rich AREA** (a feature in EXPLORE) with internal tabs — **Browse** (the existing namespaces/tables view) and **Access** (catalog tokens, moved out of the now-removed "Settings"). This is the seed of full lakekeeper-console parity (later: table-detail, tasks, permissions).
- **Storage is its own MANAGE page**, an **editable** per-workspace S3/MinIO config that re-points the workspace's lakekeeper warehouse.

This **supersedes** the just-built "Workspace Settings" shell (the `WorkspaceSettings`/`settings/*` routes + the "Settings" nav item are removed; the catalog-token UI moves into the Catalog area; the storage UI becomes the standalone editable Storage page). The **backend catalog-token routes (list/mint/revoke) stay** — only their FE home changes.

## 1. Decisions (locked)
- **Phase A (FE restructure) + Phase B (editable Storage)** in this pass. Full catalog-console parity (table detail / tasks / permissions) is a later epic.
- Storage edit re-points the warehouse via lakekeeper `update_storage_profile` + `update_storage_credential` (in-process `WarehouseService`, same seam as `create_warehouse` in `provision_ws.rs`). **Warning surfaced in UI:** changing bucket/prefix on a warehouse that already holds data orphans the old data; egret does not migrate data.
- All catalog-management + storage endpoints gated `workspace:manage_members`.

## 2. Phase A — Frontend restructure

### 2.1 Sidebar zones (`src/components/layout/Sidebar.tsx`)
- Replace the flat `EXPLORE: NavItem[]` with grouped sections: `type NavGroup = { heading: string; items: NavItem[] }`. For the workspace (`/explore/...`) render:
  - **EXPLORE**: SQL Lab, Datasets, Charts, Dashboards, Notebooks, Catalog.
  - **MANAGE**: Storage. (`{ label: "Storage", icon: HardDrive }`.)
- Remove the **"Settings"** item. Render each group with its uppercase heading (reuse the existing heading style) then its items; collapsed mode keeps icon+tooltip per item (group headings hidden when collapsed). Admin (`/admin`) nav unchanged.

### 2.2 Catalog area (`src/App.tsx` + `src/routes/explore/catalog/`)
- The `catalog` route becomes a nested area:
  ```tsx
  <Route path="catalog" element={<CatalogArea />}>
    <Route index element={<CatalogBrowse />} />
    <Route path="access" element={<CatalogAccess />} />
  </Route>
  ```
- `CatalogArea.tsx`: a header "Catalog" + an internal sub-nav (`NavLink` Browse → `.`/index, Access → `access`) + `<Outlet />`. Same sub-nav style the old `WorkspaceSettings` used.
- `CatalogBrowse.tsx`: the existing `explore/Catalog.tsx` browse content (namespaces / tables / table-detail), moved verbatim.
- `CatalogAccess.tsx`: the existing token-management page, MOVED into `src/routes/explore/catalog/` unchanged (list + mint + revoke + 403 notice).
- Delete `src/routes/explore/settings/` (WorkspaceSettings, the old StorageSettings, their tests) — replaced by the Catalog area + the Storage page.

### 2.3 Storage page (`src/App.tsx` + `src/routes/explore/StoragePage.tsx`)
- Route `<Route path="storage" element={<StoragePage />} />` (MANAGE zone).
- Phase-A content = the read view (from the old `StorageSettings`): provider/bucket/endpoint/region/key_prefix/access_key/has_secret; 404 → "platform default storage" info; 403 → admin notice. Phase B turns it editable (2.5).

## 3. Phase B — Editable Storage

### 3.1 Backend: update-storage endpoint (`egret-catalog-engine/src/browse.rs`)
- `PUT /ws/{workspace}/catalog/storage` (keep storage under the catalog router that has `CatalogStore` + the engine `ApiContext` + `WarehouseService`), gated `workspace:manage_members`. Body `UpdateStorageRequest { provider, endpoint: Option<String>, region, bucket, key_prefix: Option<String>, access_key, secret_key: Option<String>, path_style }` (`secret_key` optional — when omitted, keep the existing secret).
  - Resolve `warehouse_id = store.warehouse_for_workspace(ws)` → 409/400 if the workspace has no warehouse yet.
  - Persist egret side: if `secret_key` present → `secret_store.put(...)` → new `secret_ref`; else reuse the row's existing `secret_ref`. `store.upsert_workspace_storage(row)`.
  - Re-point the lakekeeper warehouse: build the S3 `StorageProfile` + `StorageCredential` (mirror `provision_ws.rs::build_storage_profile/credential`), call `WarehouseService::update_storage_profile(warehouse_id, UpdateWarehouseStorageRequest{…})` and `update_storage_credential(warehouse_id, UpdateWarehouseCredentialRequest{…})` via the engine `ApiContext` + a system `RequestMetadata` (same in-process pattern as provisioning). Inspect the two request struct shapes in `lakekeeper/.../v1/warehouse/mod.rs`.
  - Return `200 WorkspaceStorageInfo` (no secret).
- `GET /ws/{workspace}/storage` (existing, in the workspace router) stays the read; the new PUT lives in the catalog router (it needs the engine handle). The FE reads via GET and writes via PUT.
- **Error/UX:** if lakekeeper `update_storage_*` fails (bad creds — it validates access), return 400 with the message; do NOT leave egret's row diverged from lakekeeper (update lakekeeper FIRST, then persist the egret row + secret on success — or wrap so a lakekeeper failure aborts before the egret upsert).

### 3.2 Frontend: editable Storage form (`StoragePage.tsx`)
- An editable form (provider fixed "s3" for now; endpoint, region, bucket, key_prefix, access_key, secret_key) prefilled from `useWorkspaceStorage` (secret_key blank with "leave blank to keep current"). A "Save" button → `useUpdateStorage` mutation (`PUT …/catalog/storage`) → invalidate the storage query + success toast/inline confirmation.
- A prominent warning: "Changing the bucket or endpoint re-points this workspace's warehouse; existing data in the old location is not migrated."
- 404 (platform default) → the form is empty/defaultable so the admin can set a custom config; 403 → admin notice. Never render the current secret value.

## 4. Testing
- **BE (DB+MinIO):** `PUT …/catalog/storage` updates the `workspace_storage` row + re-points the warehouse (assert the warehouse's storage reflects the new bucket via a follow-up read / `get_warehouse`); omitting `secret_key` keeps the prior secret; a bad credential → 400 and the egret row is NOT corrupted; non-admin → 403; workspace without a warehouse → 4xx. Keep the existing token-route tests green.
- **FE (Vitest, mocked):** Sidebar renders the EXPLORE + MANAGE groups (no "Settings" item; Storage under MANAGE). CatalogArea renders Browse + Access sub-nav and routes between them; CatalogAccess still mints/lists/revokes (its existing tests move with it). StoragePage: read render (no secret); the edit form submits via `useUpdateStorage` and shows the re-point warning; 403 notice. Remove the old settings tests.

## 5. File map
- BE: `crates/egret-catalog-engine/src/browse.rs` (PUT storage handler + DTO + lakekeeper update calls) (+ test). Possibly a small `provision_ws.rs` refactor to reuse `build_storage_profile/credential` (make them `pub(crate)`).
- FE: `Sidebar.tsx` (groups); `App.tsx` (catalog nested area + storage route; drop settings); new `src/routes/explore/catalog/{CatalogArea,CatalogBrowse,CatalogAccess}.tsx` (CatalogBrowse = moved Catalog.tsx; CatalogAccess moved from settings/); `src/routes/explore/StoragePage.tsx`; `src/api/catalog.ts` (`useUpdateStorage`); delete `src/routes/explore/settings/*`; move/adjust tests.

## 6. Risks
- **Warehouse re-point semantics:** `update_storage_profile`/`update_storage_credential` validate access (live S3); a bad config fails loudly (good). Orphaned data on bucket change is a real, documented limitation — warn in UI, don't auto-migrate.
- **egret/lakekeeper consistency:** update lakekeeper first (it validates), persist egret row only on success, to avoid divergence.
- **FE move churn:** moving Catalog.tsx → CatalogBrowse + CatalogAccess across dirs must keep imports + existing browse/token tests green; the browse page behavior is unchanged.
- **Secret hygiene** unchanged: PUT accepts `secret_key` write-only; GET/Info never returns it; never logged.

## 7. Out of scope (later epics)
- Full lakekeeper-console catalog parity: table detail (schema/snapshots/files/health/versioning), namespace create/drop, generic tables, catalog Tasks tab (CE-6 view), catalog Permissions/RBAC tab, statistics.
- MANAGE zone: Members, database/engine connections, external catalog connections, workspace secrets.
- GCS/Azure storage providers (S3 only; `provider` reserved).
- Data migration on storage re-point.
