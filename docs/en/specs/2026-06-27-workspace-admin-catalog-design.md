# Workspace Admin — Catalog access + Storage (layer 2) — Design

**Date:** 2026-06-27
**Status:** Design — approved
**Branch:** `feat/workspace-admin-catalog` (off main)
**Parent:** architecture `docs/en/architecture/2026-06-17-egret-architecture-design.md` (ADR-19 three-layer management model)

## 0. Goal

Establish the **Workspace Admin** surface (ADR-19 layer 2 — per-workspace configuration, a new FE area that doesn't exist yet) and fill its first two sections: **Catalog access** (manage the workspace's catalog tokens — mint / list / revoke, with read/write + namespace scope) and **Storage** (view the workspace's storage config). These management features currently exist only as CLI / backend; this exposes them in the UI. The catalog **browse** page stays a layer-3 feature (`explore/Catalog.tsx`, untouched).

Members, database/engine connections, and external-catalog connections are later increments in the same Workspace Admin area.

## 1. Decisions (locked)

- **Placement:** a new per-workspace route group `/explore/:workspace/settings/*` (a "Settings" item in the workspace sidebar), with an internal sub-nav (Catalog access · Storage). NOT tabs inside the catalog browse page; NOT the platform `/admin/*` area.
- **Backend token routes live in `egret-catalog-engine/src/browse.rs`** — its `BrowseState` already carries the `CatalogStore` + the `WsContext` auth handles, so no new crate seam is needed (unlike storage, which used the `WarehouseProvisioner::storage_info` seam because `egret-workspace` can't depend on `egret-catalog`).
- **All management endpoints require `workspace:manage_members`** (workspace-admin), mirroring the storage GET.
- **The raw token is shown exactly once** (on mint); it is never stored in plaintext or returned again (only the SHA-256 hash is stored — unchanged).
- **403-graceful FE:** the FE has no per-workspace-role hook yet (`useMe()` carries only platform roles), so the Settings sections are always reachable; a 403 from the API renders an inline "requires workspace admin" notice rather than hiding the nav. Role-based hiding is a later enhancement (needs a per-workspace-role endpoint).

## 2. Backend

### 2.1 Token store (`egret-catalog/src/service/tokens.rs`)
`CatalogStore` already has `create_token`, `list_tokens`, `revoke_token`. Add a workspace-guarded revoke to prevent cross-workspace IDOR ([[api-nested-resource-ownership-scoping]]):
- `revoke_token_for_workspace(&self, id: Uuid, workspace_id: Uuid) -> Result<bool, CatalogError>` — `UPDATE catalog_tokens SET revoked_at = now() WHERE id = $1 AND workspace_id = $2 AND revoked_at IS NULL` → returns `true` if a row was updated, `false` otherwise (unknown id / other workspace / already revoked).

### 2.2 HTTP routes (`egret-catalog-engine/src/browse.rs`, added to the catalog router)
All gated `st.authz.require_workspace(&ctx, "workspace:manage_members")` after the `WsContext(ctx)` extractor; resolve the workspace UUID from `ctx.workspace_id`.

- `GET /ws/{workspace}/catalog/tokens` → `200 [CatalogTokenDto]` where `CatalogTokenDto { id, label, permissions: Vec<String>, namespace_scope: Vec<String>, created_at }` (from `store.list_tokens(ws)` — NEVER the hash).
- `POST /ws/{workspace}/catalog/tokens` body `MintTokenRequest { label: String, permissions: Vec<String> /* "read"|"write" */, namespace_scope: Vec<String> }`:
  - Map `"read"→"catalog:read"`, `"write"→"catalog:write"` (reject unknown values → 400). Reject empty permissions → 400.
  - `store.create_token(ws, &label, &perms, &namespace_scope)` → `200 MintTokenResponse { token: CatalogTokenDto, raw: String }` (the one-time secret).
- `DELETE /ws/{workspace}/catalog/tokens/{id}` → `store.revoke_token_for_workspace(id, ws)`: `true` → `204`; `false` → `404` (not found in this workspace — closes the IDOR).

Error mapping: reuse `BrowseApiError` (or the crate's existing error type) — 400 for bad input, 403 from the authz layer, 500 on store errors.

### 2.3 Storage
`GET /api/ws/{workspace}/storage` already exists (gated `workspace:manage_members`, returns `WorkspaceStorageInfo { provider, endpoint, region, bucket, key_prefix, access_key, has_secret }` — no secret). No backend change; the FE consumes it.

## 3. Frontend

### 3.1 Routing + nav (`src/App.tsx`, `src/components/layout/Sidebar.tsx`)
- Add to the `EXPLORE` nav array: `{ label: "Settings", icon: Settings }` (lucide `Settings`) → routes under `/explore/:workspace/settings`.
- In `ExploreRoutes`, add a nested route:
  ```tsx
  <Route path="settings" element={<WorkspaceSettings />}>
    <Route index element={<Navigate to="catalog" replace />} />
    <Route path="catalog" element={<CatalogAccess />} />
    <Route path="storage" element={<StorageSettings />} />
  </Route>
  ```
- `WorkspaceSettings` (`src/routes/explore/settings/WorkspaceSettings.tsx`): a layout with a header ("Workspace settings") + an internal sub-nav (NavLink: Catalog access → `catalog`, Storage → `storage`) + `<Outlet />`. (Sub-nav is a simple horizontal/vertical link list, not the shadcn-missing Tabs primitive — avoids adding a new UI primitive.)

### 3.2 API hooks (`src/api/catalog.ts`)
Add TanStack Query hooks (mirror the existing `useCatalogSummary` etc. + `catalogKeys`):
- Types: `CatalogTokenDto`, `MintTokenRequest`, `MintTokenResponse`, `WorkspaceStorageInfo`.
- `useCatalogTokens(ws)` → `GET /api/ws/${ws}/catalog/tokens`.
- `useMintToken(ws)` → mutation `POST …/catalog/tokens` (invalidates the tokens query on success; returns `{token, raw}`).
- `useRevokeToken(ws)` → mutation `DELETE …/catalog/tokens/${id}` (invalidates).
- `useWorkspaceStorage(ws)` → `GET /api/ws/${ws}/storage`.
- All via `apiJson` / `apiFetch` (CSRF + cookie handled). Surface `ApiError.status === 403` so pages can show the admin notice.

### 3.3 Catalog access page (`src/routes/explore/settings/CatalogAccess.tsx`)
- Token list (table: Label · Permissions (read/write badges) · Namespaces (`*` if empty) · Created · Revoke action). Loading skeleton; empty state; on `ApiError` 403 → "Requires workspace admin" inline notice; other error → generic error.
- **Mint** button → `dialog` (shadcn `Dialog`): inputs Label, permission checkboxes (Read / Write), Namespaces (comma-separated, optional). Submit → `useMintToken`. On success, the dialog switches to a **"copy this token now"** view showing the `raw` value once (monospace + copy button + a "you won't see it again" warning); closing returns to the list (now refetched).
- **Revoke** → `confirm-delete-dialog` → `useRevokeToken`.

### 3.4 Storage page (`src/routes/explore/settings/StorageSettings.tsx`)
- Read-only display of `useWorkspaceStorage(ws)`: provider, bucket, endpoint, region, key_prefix, access_key, and a "secret configured: yes/no" (`has_secret`) — NEVER a secret value. 404 → "This workspace uses the platform default storage." 403 → admin notice. A short note that storage is set at workspace creation and changing it is not yet supported (CE-5 scope).

## 4. Testing

- **Backend (DB+MinIO, extend an engine test):** mint a token via `POST` → 200 with a `raw` + the token appears in `GET` list (no hash field anywhere); revoke via `DELETE` → 204 and it disappears from the list; revoking an id from ANOTHER workspace → 404 (IDOR closed); a non-admin caller → 403; bad permission value → 400.
- **Frontend (Vitest, mock `@/api/catalog`):**
  - `WorkspaceSettings` renders the sub-nav and routes between Catalog access / Storage.
  - `CatalogAccess`: renders a mocked token list; the Mint dialog submits and shows the one-time `raw`; revoke calls the mutation; a 403 shows the admin notice.
  - `StorageSettings`: renders mocked storage info without any secret value; 404 shows the default-storage message.
  - Extend/keep `explore/__tests__/Catalog.test.tsx` (browse page) green (unchanged).

## 5. File map
- BE: `crates/egret-catalog/src/service/tokens.rs` (+ test); `crates/egret-catalog-engine/src/browse.rs` (routes + DTOs + handlers) (+ its integration test).
- FE: `src/api/catalog.ts` (hooks); `src/components/layout/Sidebar.tsx` (nav item); `src/App.tsx` (routes); new `src/routes/explore/settings/{WorkspaceSettings,CatalogAccess,StorageSettings}.tsx`; tests under `src/routes/explore/settings/__tests__/`.

## 6. Risks
- **IDOR on revoke** — closed by `revoke_token_for_workspace` (workspace-scoped UPDATE) + 404 on miss; explicitly tested.
- **Raw-token leak** — the `raw` is returned only by POST and rendered once client-side; never persisted/logged; the list/Dto never carry it.
- **No FE role hook** — Settings nav is visible to all members; the API 403 is the real gate, surfaced as an inline notice. Acceptable; role-based hiding deferred.
- **Auth pattern parity** — token handlers must use the SAME `WsContext` + `require_workspace("workspace:manage_members")` as `get_storage`/`summary`; a mistake here is a security gap (test the 403 + IDOR cases).

## 7. Out of scope (later, same area)
- Members, database/engine connections (self-service per workspace), external-catalog connections, workspace secrets — later Workspace Admin increments.
- Hiding the Settings nav by workspace role (needs a per-workspace-role endpoint).
- Editing storage of a provisioned warehouse (CE-5 limitation).
- Platform `/admin/Catalog` cross-workspace overview (layer 1).
