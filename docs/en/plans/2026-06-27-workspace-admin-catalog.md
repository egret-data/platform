# Workspace Admin — Catalog access + Storage — Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Establish the per-workspace **Workspace Admin** area (`/explore/:workspace/settings/*`, ADR-19 layer 2) with two sections — **Catalog access** (mint/list/revoke catalog tokens) and **Storage** (read-only view) — backed by new catalog-token HTTP routes.

**Architecture:** Token HTTP routes go in `egret-catalog-engine/src/browse.rs` (its `BrowseState` already has `CatalogStore` + `WsContext` auth), gated `workspace:manage_members`. The FE adds a "Settings" sidebar item → a `WorkspaceSettings` layout with a sub-nav + nested routes. The catalog **browse** page (`explore/Catalog.tsx`) is untouched.

**Tech Stack:** Rust (axum, sqlx 0.8 egret-catalog / 0.9 engine), React 18 + Vite + Tailwind v4 + Radix/shadcn + TanStack Query + react-router v6 + Vitest.

**Branch:** `feat/workspace-admin-catalog` (off main). Backend tests: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`; MinIO up. FE: `cd frontend`.

**Spec:** `docs/en/specs/2026-06-27-workspace-admin-catalog-design.md`.

**Verified seams:**
- `egret-catalog` `CatalogStore`: `create_token(ws, label, &[String] perms, &[String] ns) -> (CatalogToken, raw)`, `list_tokens(ws) -> Vec<CatalogToken>`, `revoke_token(id)`. `CatalogToken { id, workspace_id, label, permissions, namespace_scope, created_at }` (no hash). Perms validated as `catalog:read`/`catalog:write`.
- `egret-catalog-engine/src/browse.rs`: `BrowseState { engine, store: CatalogStore, system_user, sessions, workspaces, authz }`; router `.route("/ws/{workspace}/catalog", get(summary))` etc. (mounted at `/api`). Handlers use `WsContext(ctx)` + `authorized_warehouse(&st,&ctx)`. Mirror `get_storage`'s gate: `st.authz.require_workspace(&ctx, "workspace:manage_members").await`.
- `GET /api/ws/{workspace}/storage` exists (`WorkspaceStorageInfo { provider, endpoint, region, bucket, key_prefix, access_key, has_secret }`).
- FE: `src/api/client.ts` `apiJson<T>(path, init)` / `apiFetch` (CSRF + cookie for mutating). `src/api/catalog.ts` (existing hooks + `catalogKeys`). `src/App.tsx` `ExploreRoutes` (nested `<Routes>`). `src/components/layout/Sidebar.tsx` `EXPLORE: NavItem[]` (label+icon → `slug(label)` route). shadcn available: `dialog`, `confirm-delete-dialog`, `alert`, `badge`, `button`, `input`, `card`, `select` (NO `tabs`/`table` primitive — use plain markup). `useMe()` in `src/api/hooks.ts` (platform roles only).

---

## Task 1: Backend — catalog token routes

**Files:** `crates/egret-catalog/src/service/tokens.rs` (+ test `tests/store_tokens.rs`); `crates/egret-catalog-engine/src/browse.rs` (+ its integration test, e.g. extend `tests/front_auth.rs` or a new `tests/catalog_tokens_api.rs`).

- [ ] **Step 1 (store guard + test):** in `tokens.rs` add `pub async fn revoke_token_for_workspace(&self, id: Uuid, workspace_id: Uuid) -> Result<bool, CatalogError>` running `UPDATE catalog_tokens SET revoked_at = now() WHERE id = $1 AND workspace_id = $2 AND revoked_at IS NULL` and returning `result.rows_affected() == 1`. In `tests/store_tokens.rs` add: create a token in ws A → `revoke_token_for_workspace(id, A)` returns `true` and it's gone from `list_tokens(A)`; `revoke_token_for_workspace(id, B)` (other ws) returns `false`; revoking again returns `false`. Run `cargo test -p egret-catalog --test store_tokens` → PASS.

- [ ] **Step 2 (DTOs + handlers, failing integration test first):** in `browse.rs` add serde DTOs:
  ```rust
  #[derive(Serialize)] struct CatalogTokenDto { id: Uuid, label: String, permissions: Vec<String>, namespace_scope: Vec<String>, created_at: chrono::DateTime<chrono::Utc> }
  #[derive(Deserialize)] struct MintTokenRequest { label: String, permissions: Vec<String>, namespace_scope: Vec<String> }
  #[derive(Serialize)] struct MintTokenResponse { token: CatalogTokenDto, raw: String }
  ```
  Write the integration test first (study `tests/front_auth.rs` for mounting the browse router + a workspace-admin session/cookie; if the harness for the browse router + an admin WsContext is heavy, reuse the exact setup the existing browse tests use): mint via POST → 200 with non-empty `raw`; GET list contains the token (assert NO `token_hash`/`hash`/secret in the JSON body bytes); DELETE → 204 and it's gone; DELETE an id created in another workspace → 404; a non-admin member → 403; `permissions: ["bogus"]` → 400. Run → FAIL.

- [ ] **Step 3 (handlers + routes):** add to the catalog router:
  - `GET /ws/{workspace}/catalog/tokens`: `WsContext(ctx)` → `require_workspace(&ctx,"workspace:manage_members")` → `store.list_tokens(ctx.workspace_id)` → map to `Vec<CatalogTokenDto>` → `Json`.
  - `POST /ws/{workspace}/catalog/tokens`: gate as above; map each `permissions` entry `"read"→"catalog:read"`, `"write"→"catalog:write"`, anything else → 400; empty → 400; `store.create_token(...)` → `Json(MintTokenResponse{ token: dto, raw })`.
  - `DELETE /ws/{workspace}/catalog/tokens/{id}`: gate; `store.revoke_token_for_workspace(id, ctx.workspace_id)` → `true` ⇒ `StatusCode::NO_CONTENT`, `false` ⇒ 404.
  Use the crate's existing error type for 400/403/500 (the authz error already maps to 403). Run the integration test → PASS.

- [ ] **Step 4:** `cargo test -p egret-catalog -p egret-catalog-engine`; `cargo fmt --all`; `cargo clippy -p egret-catalog -p egret-catalog-engine --all-targets -- -D warnings`; single iceberg. Commit `feat(catalog): workspace-admin catalog token HTTP routes (list/mint/revoke)`.

---

## Task 2: Frontend — Workspace Admin shell + API hooks

**Files:** `frontend/src/api/catalog.ts`; `frontend/src/components/layout/Sidebar.tsx`; `frontend/src/App.tsx`; new `frontend/src/routes/explore/settings/WorkspaceSettings.tsx`; test `frontend/src/routes/explore/settings/__tests__/WorkspaceSettings.test.tsx`.

- [ ] **Step 1 (api hooks):** in `catalog.ts` add types (`CatalogTokenDto`, `MintTokenRequest`, `MintTokenResponse`, `WorkspaceStorageInfo`) and hooks, mirroring the existing query style + extending `catalogKeys`:
  - `useCatalogTokens(ws)` = `useQuery` `GET /api/ws/${ws}/catalog/tokens`.
  - `useMintToken(ws)` = `useMutation` `POST /api/ws/${ws}/catalog/tokens` (body JSON), `onSuccess` invalidate the tokens key; returns `MintTokenResponse`.
  - `useRevokeToken(ws)` = `useMutation` `DELETE /api/ws/${ws}/catalog/tokens/${id}`, invalidate.
  - `useWorkspaceStorage(ws)` = `useQuery` `GET /api/ws/${ws}/storage`.
  Use `apiJson`/`apiFetch`; let `ApiError` propagate (pages read `.status === 403`).

- [ ] **Step 2 (nav + routes):** add `{ label: "Settings", icon: Settings }` (import `Settings` from lucide-react) to the `EXPLORE` array in `Sidebar.tsx`. In `App.tsx` `ExploreRoutes`, add the nested route block (import the three new components):
  ```tsx
  <Route path="settings" element={<WorkspaceSettings />}>
    <Route index element={<Navigate to="catalog" replace />} />
    <Route path="catalog" element={<CatalogAccess />} />
    <Route path="storage" element={<StorageSettings />} />
  </Route>
  ```

- [ ] **Step 3 (shell):** `WorkspaceSettings.tsx`: a header "Workspace settings" + a sub-nav of `NavLink`s (`Catalog access` → `catalog`, `Storage` → `storage`, relative paths; active styling like the spec) + `<Outlet />`. Create minimal `CatalogAccess.tsx` / `StorageSettings.tsx` stubs (just a heading) so routing compiles — Tasks 3 & 4 fill them.

- [ ] **Step 4 (test):** `WorkspaceSettings.test.tsx` — render within a `MemoryRouter` at `/explore/w1/settings/catalog`, assert the sub-nav links render and the active section shows. `cd frontend && npm run lint && npm run typecheck && npm test && npm run build`. Commit `feat(web): workspace settings shell + catalog/storage routes + api hooks`.

---

## Task 3: Frontend — Catalog access page

**Files:** `frontend/src/routes/explore/settings/CatalogAccess.tsx`; test `__tests__/CatalogAccess.test.tsx`.

- [ ] **Step 1 (list + states):** render `useCatalogTokens(ws)` as a table (plain markup: Label · Permissions as Read/Write `badge`s · Namespaces (`*` when empty, else comma-joined) · Created (RelativeTime if available) · a Revoke button per row). Loading → skeleton; empty → "No catalog tokens yet"; `ApiError.status===403` → an `alert` "Requires workspace admin to manage catalog access"; other error → generic error alert.

- [ ] **Step 2 (mint dialog):** a "Mint token" `button` opens a shadcn `Dialog` with: `input` Label, two checkboxes Read / Write (at least one required), `input` Namespaces (comma-separated, optional → split/trim/drop-empty). Submit → `useMintToken`. On success, swap the dialog body to a **one-time reveal**: the `raw` value in a monospace box + a Copy button (navigator.clipboard) + a warning "Copy it now — it won't be shown again." Closing the dialog refetches/returns to the list.

- [ ] **Step 3 (revoke):** Revoke button → `confirm-delete-dialog` (existing component) → `useRevokeToken(ws)` with the row id.

- [ ] **Step 4 (tests):** mock `@/api/catalog`. Assert: list renders mocked tokens with permission badges + namespaces; opening Mint, checking Read, typing a label, submitting calls the mint mutation and then shows the `raw` once (and the raw is NOT shown before mint); revoke triggers the mutation; a 403 query error renders the admin notice. `npm run lint && npm run typecheck && npm test && npm run build`. Commit `feat(web): catalog access page — mint/list/revoke catalog tokens`.

---

## Task 4: Frontend — Storage page + final review

**Files:** `frontend/src/routes/explore/settings/StorageSettings.tsx`; test `__tests__/StorageSettings.test.tsx`.

- [ ] **Step 1 (display):** render `useWorkspaceStorage(ws)` read-only in a `card`: provider, bucket, endpoint (or "AWS default" when null), region, key_prefix (or default note), access_key, and "Secret configured: Yes/No" from `has_secret`. NEVER render a secret value. `ApiError.status===404` → "This workspace uses the platform default storage." `403` → admin notice. Add a muted note: "Storage is set when the workspace is created; changing it isn't supported yet."

- [ ] **Step 2 (tests):** mock `@/api/catalog`. Assert: renders mocked storage info, shows "Secret configured: Yes" for `has_secret:true`, and the raw secret string never appears in the DOM; a 404 shows the default-storage message; a 403 shows the admin notice.

- [ ] **Step 3 (verify):** `cd frontend && npm run lint && npm run typecheck && npm test && npm run build`. Confirm the existing `explore/__tests__/Catalog.test.tsx` (browse) still passes. Commit `feat(web): workspace storage settings page (read-only)`.

---

## Self-review checklist
- [ ] BE: token routes gated `workspace:manage_members`; mint returns `raw` once; list/Dto carry NO hash; revoke is workspace-scoped (IDOR → 404); bad perms → 400.
- [ ] FE: "Settings" nav → WorkspaceSettings shell with Catalog access + Storage sub-nav; browse page untouched.
- [ ] Mint shows the raw token exactly once + copy; revoke confirms; 403 → inline admin notice; storage read-only, no secret rendered.
- [ ] Backend suite + `cargo clippy` green, single iceberg; `npm run lint/typecheck/test/build` green.

## Out of scope
Members / DB connections / external catalogs / workspace secrets (later same area); role-based nav hiding; editing provisioned storage; platform admin/Catalog overview.
