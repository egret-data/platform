# Catalog-per-workspace — Plan A-3: token auth + RBAC + internal API + Catalog UI

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`).

**Goal:** Close the auth/UX gaps left by A-1/A-2: (a) replace the static catalog bearer token with per-workspace `catalog_tokens` and enforce that an engine token can only touch its own workspace's `{prefix}` (403 otherwise), tightening `sign`/`metrics`; (b) add RBAC `catalog:read|write` perms; (c) a session-authenticated internal API for the UI to browse a workspace's catalog; (d) a read-only Catalog view in the workspace/explore area (moved out of platform admin).

**Architecture:** Two auth planes. **Engines** (Daft/Trino) authenticate with a hashed per-workspace token; the catalog middleware authenticates the token → workspace, stashes it in request extensions, and `resolve_catalog` enforces `token.workspace == prefix.workspace` (403 on mismatch). **UI** uses the egret session JWT → `WorkspaceContext` → `Authorizer.require_workspace(ctx, "catalog:read")`, hitting a NEW internal API (`/api/...`) that reads the normalized catalog rows directly — never the Iceberg REST surface.

**Tech Stack:** Rust (axum, sqlx, sha2), React + Vite + TanStack Query (match existing `frontend/src`).

**Base:** branch from `main` (A-2 merged at `bc50a9b`). DB tests: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`.

**Spec:** `docs/superpowers/specs/2026-06-26-catalog-per-workspace-design.md` §2.4, §2.5, §5, §6.

**Scope:** This finishes spec A. NOT here: epic parts B (storage/file-browser), C (notebook core), D (Daft kernel), E (execution). Per-workspace MinIO IAM credential isolation is part D.

---

## Phase 0 — RBAC perms

### Task 1: seed `catalog:read` / `catalog:write`
**Files:** new `migrations/0024_catalog_perms.sql`; touch `crates/egret-metastore/src/lib.rs` to re-embed.
- [ ] Read `migrations/0003_workspace_rbac.sql` + a feature seed (e.g. `0009_engine.sql`) to learn the exact roles table + `role_permissions` shape and the existing workspace role names (e.g. `owner`/`editor`/`viewer` — confirm).
- [ ] New migration inserts `catalog:read` into read-capable workspace roles and `catalog:write` into write-capable roles, matching the existing INSERT idiom (`INSERT INTO role_permissions (role_id, permission) SELECT id, 'catalog:read' FROM roles WHERE scope='workspace' AND name IN (...)`). Idempotent (`ON CONFLICT DO NOTHING` if the table has a PK/unique).
- [ ] Touch metastore lib.rs (re-embed gotcha). Verify migration applies against a fresh scratch DB. Commit `feat(authz): seed catalog:read/write workspace permissions`.

---

## Phase 1 — Engine token auth (`catalog_tokens`)

### Task 2: token hashing + store ops
**Files:** `crates/egret-catalog/src/service/tokens.rs` (new), wire in `service/mod.rs`; test `tests/store_tokens.rs`.
- [ ] `catalog_tokens` table (from A-1 migration 0023): `{id, workspace_id, token_hash, label, created_at, revoked_at}`, UNIQUE(token_hash).
- [ ] Add (use the `sha2` crate — check it's a workspace dep, else add): `fn hash_token(raw: &str) -> String` (hex SHA-256). Store methods on `CatalogStore`: `create_token(workspace_id, label) -> (token_id, raw_token)` (generate a random token, store only its hash, return the raw ONCE), `workspace_for_token(raw: &str) -> Result<Uuid, CatalogError>` (hash, look up an unrevoked row, return workspace_id or `Unauthorized`/NotFound), `revoke_token(id)`.
- [ ] Random token generation: use a CSPRNG (e.g. `rand` if a workspace dep, or read 32 bytes and hex). Confirm an existing crypto dep in `egret-secrets`/`egret-auth` to reuse rather than adding one.
- [ ] TDD `store_tokens.rs`: create → `workspace_for_token(raw)` returns the workspace; a wrong/revoked token → error; the stored value is a hash (not the raw). Commit `feat(catalog): per-workspace catalog token store (hashed)`.

### Task 3: `require_bearer` authenticates a catalog token; `resolve_catalog` enforces prefix ownership
**Files:** `crates/egret-catalog/src/auth.rs`, `src/api/resolve.rs`, handlers (`sign`/`metrics`).
- [ ] **Middleware (authn):** rewrite `require_bearer` to: extract the Bearer token, call `state.service.store.workspace_for_token(token)`. On success, insert an `AuthnWorkspace(Uuid)` (new newtype) into `req.extensions_mut()` and continue; on failure → 401. (Decide whether to also keep the static `config.catalog.auth_token` as an admin/superuser bypass that sets a "platform" marker — RECOMMEND: keep it as an admin override that passes any prefix check, for ops/tests; document it.)
- [ ] **`resolve_catalog` (authz):** it already resolves the prefix→catalog. Add: read the `AuthnWorkspace` from request extensions (thread it via an extractor or pass it in) and enforce `authn_workspace == catalog.workspace_id` → else `CatalogError::Forbidden` (403). The admin-override marker bypasses this. Add `CatalogError::Forbidden → 403` to `http_error.rs` if absent.
- [ ] **Tighten `sign` + `metrics`:** make both resolve the catalog (so a bogus/unauthorized prefix → 404/403) instead of ignoring the prefix. `sign` still uses `storage_cfg` for signing but must pass the ownership check first.
- [ ] TDD (in `http_roundtrip.rs` or a new auth test): a token for workspace A hitting prefix B → 403; an unknown token → 401; the right token → 200. Commit `feat(catalog): enforce per-workspace token ownership on {prefix}`.

### Task 4: mint/revoke tokens (minimal)
**Files:** a CLI subcommand in `egret-bin` OR a superadmin HTTP endpoint — pick the smaller.
- [ ] Provide a way to mint a token for a workspace and print the raw value once (e.g. `egret catalog mint-token --workspace <slug>`), and revoke by id. Keep it minimal; this is ops tooling. Wire `egret-bin` to construct `CatalogStore` for it. TDD the store path is already covered (Task 2); a thin CLI test or manual note suffices. Commit `feat(cli): mint/revoke per-workspace catalog tokens`.

---

## Phase 2 — Internal UI API (session + RBAC)

### Task 5: catalog read API for the UI
**Files:** new `crates/egret-catalog/src/api/internal.rs` (or in `egret-server`); wire a router mounted under the session-authenticated `/api` tree; reuse the `WorkspaceContext` extractor + `Authorizer`.
- [ ] Read how existing workspace-scoped APIs are mounted + how `WorkspaceContext` / `Authorizer.require_workspace` are used (e.g. egret-bi or egret-engine routes, `egret-workspace/src/routes.rs`, `egret-server/src/lib.rs`).
- [ ] Endpoints (all `require_workspace(ctx, "catalog:read")`), reading the NORMALIZED rows (not Iceberg REST):
  - `GET /api/workspaces/:ws/catalog` → `{ name, warehouse_location }`.
  - `GET /api/workspaces/:ws/catalog/namespaces` → `[levels...]`.
  - `GET /api/workspaces/:ws/catalog/namespaces/:ns/tables` → `[name...]` (and `/views`).
  - `GET /api/workspaces/:ws/catalog/namespaces/:ns/tables/:t` → table detail: current schema (columns), snapshot count / current snapshot, properties — from `catalog_table_*` rows (add small read-only store methods as needed).
- [ ] Resolve the catalog from `:ws` (the path workspace) via `get_catalog_by_id`/`by_workspace`; ensure `ctx.workspace_id == :ws` (the `WorkspaceContext` already binds the active workspace — enforce they match or derive from ctx).
- [ ] TDD: a member with `catalog:read` lists namespaces/tables; a non-member → 403. Commit `feat(catalog): internal session API for catalog browsing`.

---

## Phase 3 — Frontend Catalog view (workspace/explore)

### Task 6: move Catalog into the workspace area + wire the API
**Files:** `frontend/src/App.tsx` (route), `frontend/src/components/layout/Sidebar.tsx` (already lists "Catalog" — confirm it points at the workspace base), `frontend/src/routes/explore/Catalog.tsx` (new, replacing the admin placeholder usage), `frontend/src/api/*` (client).
- [ ] Read `frontend/src/App.tsx` (workspace `Routes` vs `AdminRoutes`), `Sidebar.tsx`, an existing explore route (e.g. `Datasets.tsx`) for the data-fetching pattern (TanStack Query? the `api/` clients), and `routes/admin/Catalog.tsx` (current placeholder).
- [ ] Add a workspace route `catalog` (in the same `Routes` block as `sql-lab`/`datasets`/`notebooks`), pointing to a new `routes/explore/Catalog.tsx`. Remove/retire the admin Catalog placeholder route (or leave admin's pointing to a platform overview — minimal: just add the workspace one and update the sidebar link).
- [ ] `Catalog.tsx`: read-only browser — list namespaces → tables/views → on select, show table detail (schema columns, snapshot info, properties) via the Task 5 API. Match the existing explore pages' look (shadcn components, OKLCH tokens — use `var(--token)` directly, never `hsl(var(--token))`).
- [ ] Frontend tests (Vitest, matching existing `__tests__`): render lists from a mocked API; basic interaction. Run `npm run test` + `npm run build` + lint. Commit `feat(frontend): workspace Catalog browser`.

---

## Self-review checklist
- [ ] Engine token: hashed at rest; A's token on B's prefix → 403; unknown token → 401; static admin override documented (or removed).
- [ ] `sign` + `metrics` now enforce prefix ownership.
- [ ] `catalog:read|write` seeded; UI API gated by `require_workspace`; non-member → 403.
- [ ] Internal API reads normalized rows; never proxies Iceberg REST.
- [ ] Catalog UI is in the workspace/explore group, read-only, matches existing pages.
- [ ] `cargo test` (catalog+server+workspace) green; clippy+fmt clean; `npm run test`+`build` green.

## Out of scope (epic parts B–E)
Storage/file-browser, notebook core, Daft kernel runtime, execution wiring, per-user MinIO IAM.
