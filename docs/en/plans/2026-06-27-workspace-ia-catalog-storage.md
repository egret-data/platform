# Workspace IA restructure — Catalog area + editable Storage (Phase A+B) — Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Restructure the workspace UI (ADR-19a): sidebar zones (EXPLORE / MANAGE); Catalog becomes a rich AREA with **Browse** + **Access (tokens)** tabs; Storage becomes its own MANAGE page that is **editable** (re-points the workspace warehouse). The backend catalog-token routes already exist; this changes their FE home and adds a storage-update endpoint.

**Architecture:** FE: grouped sidebar; nested `catalog` route (CatalogArea → CatalogBrowse + CatalogAccess); standalone `storage` route; the old `settings/*` shell is deleted. BE: a `PUT /ws/{ws}/catalog/storage` handler that persists the egret `workspace_storage` row + secret AND re-points the lakekeeper warehouse via `WarehouseService::update_storage_profile`/`update_storage_credential`.

**Tech Stack:** React 18 + Vite + Tailwind v4 + shadcn + TanStack Query + react-router v6 + Vitest; Rust (axum, lakekeeper embed, egret-secrets).

**Branch:** `feat/workspace-admin-catalog` (continues — BE token routes already here). FE from `frontend/`; BE tests need DB+MinIO.

**Spec:** `docs/en/specs/2026-06-27-workspace-ia-catalog-storage-design.md`.

**Verified seams:**
- `Sidebar.tsx`: flat `EXPLORE: NavItem[]` ({label,icon}); `slug(label)` → `/explore/{ws}/{slug}`; single heading. Make it grouped.
- `App.tsx` `ExploreRoutes`: flat `<Routes>` with `catalog` + the `settings` nested block (added earlier) + the per-feature routes. `Navigate` imported.
- Existing FE: `src/routes/explore/Catalog.tsx` (browse, uses `useCatalog*` hooks); `src/routes/explore/settings/{WorkspaceSettings,CatalogAccess,StorageSettings}.tsx` + their tests; `src/api/catalog.ts` (hooks incl. `useWorkspaceStorage`, `useCatalogTokens`…).
- BE: `egret-catalog-engine/src/browse.rs` `BrowseState { engine: ApiContext<EngineState>, store: CatalogStore, system_user, sessions, workspaces, authz }`; existing token routes + `require_workspace_admin`. `CatalogStore::{warehouse_for_workspace, upsert_workspace_storage, workspace_storage_for}`. `provision_ws.rs::{build_storage_profile, build_storage_credential}` (make `pub(crate)`), `system_request_metadata`. lakekeeper `WarehouseService::{update_storage_profile(warehouse_id, UpdateWarehouseStorageRequest), update_storage_credential(warehouse_id, UpdateWarehouseCredentialRequest)}` (in `lakekeeper::api::management::v1::warehouse`; inspect the two request structs).

---

## Task 1: Sidebar zones (FE)
**Files:** `frontend/src/components/layout/Sidebar.tsx`; touch its test if any.
- [ ] Introduce `type NavGroup = { heading: string; items: NavItem[] }`. For workspace nav define `EXPLORE_GROUPS: NavGroup[] = [{ heading:"Explore", items:[SQL Lab, Datasets, Charts, Dashboards, Notebooks, Catalog] }, { heading:"Manage", items:[{label:"Storage", icon: HardDrive}] }]`. Remove the `Settings` item + import. Keep `ADMIN` as-is (one group "Administration").
- [ ] Render: iterate groups; each renders its uppercase heading (when not collapsed) then its items (the existing item/tooltip rendering). Collapsed mode: hide headings, keep icon+tooltip items.
- [ ] `npm run lint && npm run typecheck && npm test && npm run build`. Commit `feat(web): group workspace sidebar into Explore/Manage zones`.

## Task 2: Catalog area with Browse + Access (FE)
**Files:** new `frontend/src/routes/explore/catalog/{CatalogArea,CatalogBrowse,CatalogAccess}.tsx`; `App.tsx`; delete `frontend/src/routes/explore/settings/{WorkspaceSettings,CatalogAccess}.tsx` + their tests (StorageSettings handled in Task 3); test `catalog/__tests__/CatalogArea.test.tsx`; move the CatalogAccess test.
- [ ] `CatalogBrowse.tsx` = the current `explore/Catalog.tsx` content moved verbatim (keep its hooks/imports; update relative import paths). Keep `explore/Catalog.tsx`? No — replace its usages. Update the existing `explore/__tests__/Catalog.test.tsx` to target the moved component (or move it under `catalog/__tests__`).
- [ ] `CatalogAccess.tsx` = move `settings/CatalogAccess.tsx` here unchanged; move its test.
- [ ] `CatalogArea.tsx`: header "Catalog" + sub-nav (`NavLink` Browse → `.` end, Access → `access`) + `<Outlet/>` (mirror the old WorkspaceSettings sub-nav styling).
- [ ] `App.tsx`: replace the `catalog` route + remove the `settings` nested block + the `WorkspaceSettings` import. New:
  ```tsx
  <Route path="catalog" element={<CatalogArea />}>
    <Route index element={<CatalogBrowse />} />
    <Route path="access" element={<CatalogAccess />} />
  </Route>
  ```
- [ ] Delete `settings/WorkspaceSettings.tsx` + its test. (StorageSettings stays until Task 3 moves it.)
- [ ] Lint/typecheck/test/build green. Commit `feat(web): Catalog area with Browse + Access tabs (tokens move into Catalog)`.

## Task 3: Storage standalone page — read (FE)
**Files:** new `frontend/src/routes/explore/StoragePage.tsx` (+ test); `App.tsx`; delete `settings/StorageSettings.tsx` + its test + the now-empty `settings/` dir.
- [ ] `StoragePage.tsx` = the old `StorageSettings` read content (provider/bucket/endpoint/region/key_prefix/access_key/has_secret; 404→"platform default storage"; 403→admin notice; no secret rendered). Header "Storage".
- [ ] `App.tsx`: add `<Route path="storage" element={<StoragePage />} />`. Remove the old `settings/storage` route.
- [ ] Move/adapt the StorageSettings test → `explore/__tests__/StoragePage.test.tsx`. Delete `settings/`.
- [ ] Lint/typecheck/test/build green. Commit `feat(web): standalone Storage page in the Manage zone (read)`.

## Task 4: Backend — update-storage endpoint (BE)
**Files:** `crates/egret-catalog-engine/src/browse.rs` (+ test); `provision_ws.rs` (make `build_storage_profile`/`build_storage_credential` `pub(crate)`).
- [ ] **Failing integration test** (DB+MinIO): provision a workspace+warehouse; `PUT /ws/{ws}/catalog/storage` with a valid MinIO config (same endpoint, a distinct bucket that exists, valid creds) → 200; assert the `workspace_storage` row updated + the warehouse's storage reflects the new config (read back via `WarehouseService::get_warehouse` or the storage GET); omitting `secret_key` keeps the prior secret; a bogus credential → 400 (and the egret row unchanged); non-admin → 403; a workspace with no warehouse → 4xx.
- [ ] **Handler:** `PUT /ws/{workspace}/catalog/storage`, gate `require_workspace_admin`. Body `UpdateStorageRequest { provider, endpoint:Option<String>, region, bucket, key_prefix:Option<String>, access_key, secret_key:Option<String>, path_style }`.
  1. `warehouse_id = store.warehouse_for_workspace(ws)?` else 409.
  2. Resolve the effective secret: `secret_key` present → use it; else `secret_store.get(existing row.secret_ref)`.
  3. Build `StorageProfile` (S3) + `StorageCredential` via the `pub(crate)` `build_storage_profile/credential` (or inline) from the request.
  4. Call `WarehouseService::update_storage_profile(warehouse_id, UpdateWarehouseStorageRequest{ storage_profile, … })` then `update_storage_credential(warehouse_id, UpdateWarehouseCredentialRequest{ storage_credential, … })` via `engine` + `system_request_metadata` (inspect the two request struct fields in lakekeeper). **lakekeeper first** (it validates access).
  5. On success: if `secret_key` present → `secret_store.put(...)` → new `secret_ref`; `store.upsert_workspace_storage(row)`. Return `WorkspaceStorageInfo`.
  Map a lakekeeper validation failure → 400 with its message; ensure the egret row is only written after lakekeeper succeeds.
- [ ] Tests green; `cargo fmt`; `cargo clippy -p egret-catalog-engine --all-targets -D warnings`; single iceberg. Commit `feat(catalog-engine): PUT workspace storage — re-point warehouse storage`.

## Task 5: Frontend — editable Storage form (FE)
**Files:** `frontend/src/routes/explore/StoragePage.tsx`; `src/api/catalog.ts` (`useUpdateStorage`); test.
- [ ] `useUpdateStorage(ws)` mutation → `PUT /ws/${ws}/catalog/storage` (JSON body); invalidate `catalogKeys.storage(ws)` on success.
- [ ] `StoragePage` gains an edit form prefilled from `useWorkspaceStorage` (secret_key blank, placeholder "leave blank to keep current"). Fields: endpoint, region, bucket, key_prefix, access_key, secret_key (+ provider fixed "s3"). A prominent warning callout: "Changing the bucket or endpoint re-points this workspace's warehouse; existing data in the old location is not migrated." Save → `useUpdateStorage` → success inline confirmation; error → message. 404 → empty form to set a custom config; 403 → admin notice. Never prefill/render the current secret.
- [ ] Tests: form prefilled (no secret), submit calls `useUpdateStorage` with the field values, warning shown, 403 notice. Lint/typecheck/test/build green. Commit `feat(web): editable workspace Storage config`.

## Self-review checklist
- [ ] Sidebar: Explore + Manage zones; no "Settings" item; Storage under Manage.
- [ ] Catalog area: Browse + Access tabs; tokens UI works as before (moved, not regressed); browse unchanged.
- [ ] Storage page: read shows config (no secret); editable form re-points the warehouse via the BE; warning shown.
- [ ] BE: PUT gated admin; lakekeeper re-point validated-first; secret optional (keep-current); no egret/lakekeeper divergence on failure; IDOR-safe (workspace-scoped warehouse).
- [ ] `settings/*` removed; all moved tests pass; backend + `cargo clippy` green; single iceberg; FE lint/typecheck/test/build green.

## Out of scope
Full catalog-console parity (table detail/tasks/permissions); Members/connections in Manage; GCS/Azure; data migration on re-point.
