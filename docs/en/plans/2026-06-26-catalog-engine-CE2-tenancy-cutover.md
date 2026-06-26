# Catalog Engine CE-2 — Tenancy + provisioning + cutover — Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement task-by-task. Steps use `- [ ]`. Some tasks (browse rewire) are discovery against lakekeeper's in-process API — port from the referenced lakekeeper files + gate with the stated tests; do not fabricate API calls.

**Goal:** egret serves the embedded lakekeeper catalog per-workspace — git-pinned deps, real-admin bootstrap, a warehouse provisioned per workspace, the built-in `iceberg_router` replaced by lakekeeper, and the internal browse API/UI reading lakekeeper data.

**Architecture:** A new `workspace_warehouse(workspace_id, warehouse_id)` maps each egret workspace to a lakekeeper warehouse (1 default project). egret-server mounts lakekeeper's `new_v1_full_router` under `/catalog/v1` with egret `require_bearer` in front (preserving A-3's per-workspace token ownership, now keyed on warehouse UUID) and a `from_fn` principal-injection layer; AllowAll inside (real scope = CE-3/CE-4). The browse API reads lakekeeper via its in-process catalog Service. egret's superseded `catalog_*` schema + store/handlers are **deleted at the end of CE-2** (CE-8 folded in), keeping only the reusable A-3 identity layer (`catalog_tokens`, RBAC, browse, UI).

**Tech Stack:** Rust, axum, sqlx (0.8 egret / 0.9 lakekeeper engine crate), embedded `lakekeeper` (git-pinned), MinIO/S3.

**Branch:** `feat/catalog-engine-lakekeeper`. DB: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`. MinIO `egret-dev-minio-1`.

**Spec:** `docs/en/specs/2026-06-26-catalog-engine-CE2-tenancy-cutover-design.md`. **Findings:** `docs/en/specs/2026-06-26-catalog-engine-CE1-findings.md`.

**Key existing seams:**
- `crates/egret-server/src/lib.rs:94-107` — the `iceberg_router` (`/catalog`) + `browse_router` (`/api`) mounts to change.
- `crates/egret-workspace/src/workspaces.rs` — `WorkspaceRepo::create` (the A-2 provisioning hook + `warehouse_base`).
- `crates/egret-catalog/src/browse.rs` — `authorized_catalog`/`list_namespaces`/`list_tables`/`table_detail` reads (rewire targets).
- `crates/egret-catalog-engine/src/provision.rs` — `bootstrap` + `create_warehouse` (reuse).
- `crates/egret-catalog-engine/src/mount.rs` — `build_state` + `catalog_engine_router` (reuse).

---

## Phase 0 — Deps cleanup

### Task 1: git-pin lakekeeper, vendor `.sqlx`, test-utils → dev
**Files:** `crates/egret-catalog-engine/Cargo.toml`, `.cargo/config.toml`, vendored `.sqlx` cache.
- [ ] **Step 1:** Pick a lakekeeper upstream rev whose `iceberg` pin == `bb70173` (check `lakekeeper/Cargo.toml` of the current sibling checkout = the rev to pin). Replace the three `path = "../../../lakekeeper/..."` deps with `git = "https://github.com/lakekeeper/lakekeeper", rev = "<that-rev>"`. Move the `lakekeeper` `test-utils` feature to `[dev-dependencies]`.
- [ ] **Step 2:** Vendor lakekeeper's sqlx cache: copy `lakekeeper/.sqlx` → `crates/egret-catalog-engine/.sqlx-lakekeeper/` (commit it), and set `.cargo/config.toml` `SQLX_OFFLINE_DIR` to that vendored path (relative). Remove the sibling-relative `../lakekeeper/.sqlx`.
- [ ] **Step 3:** Because `test_user` (test-utils) is no longer in prod deps, `provision.rs::ensure_spike_warehouse`'s bootstrap will not compile — temporarily keep it working by feature-gating that bootstrap path under `#[cfg(test)]`/dev, OR leave `ensure_spike_warehouse` only used by the round-trip test (which can enable test-utils as a dev-dep). The PROD bootstrap is rebuilt in Task 3. Keep CE-1 tests compiling.
- [ ] **Step 4 (GATE):** `cargo build -p egret-catalog-engine` clean; `grep -c '^name = "iceberg"$' Cargo.lock` == 1; `cargo test -p egret-catalog-engine` (migrate + roundtrip) still pass. Commit `chore(catalog-engine): git-pin lakekeeper + vendor sqlx cache + test-utils to dev`.

---

## Phase 1 — Provisioning

### Task 2: `workspace_warehouse` table + store
**Files:** new `migrations/0025_workspace_warehouse.sql` + touch `crates/egret-metastore/src/lib.rs`; `crates/egret-catalog-engine/src/warehouse_map.rs`; test.
- [ ] **Step 1:** Migration:
```sql
CREATE TABLE workspace_warehouse (
  workspace_id uuid PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
  warehouse_id uuid NOT NULL,
  created_at   timestamptz NOT NULL DEFAULT now()
);
```
Touch metastore lib.rs (re-embed). Reset `egret_test` so it re-migrates.
- [ ] **Step 2:** `warehouse_map.rs` on an egret (sqlx-0.8) pool: `upsert_warehouse(workspace_id, warehouse_id)` (`INSERT ... ON CONFLICT (workspace_id) DO NOTHING`), `warehouse_for_workspace(workspace_id) -> Option<Uuid>`, `warehouse_for_slug(slug) -> Option<Uuid>` (join `workspaces`). (Egret-side mapping uses egret's own pool, NOT the lakekeeper sqlx-0.9 pool.)
- [ ] **Step 3 (TDD):** test (DB-gated): seed a workspace, upsert a warehouse_id, assert `warehouse_for_workspace` + `warehouse_for_slug` resolve it; idempotent on repeat. Run; commit `feat(catalog-engine): workspace→warehouse mapping table + store`.

### Task 3: real-admin bootstrap (replace test_user)
**Files:** `crates/egret-catalog-engine/src/provision.rs`; `crates/egret-config` (add `catalog.system_user`).
- [ ] **Step 1:** Add config `catalog.system_user` (default e.g. `"egret-system"`). Build a stable `RequestMetadata` carrying a deterministic `UserId` derived from `catalog.system_user` (e.g. `oidc~egret-system` → a fixed UUID via name hashing, the same each boot) — NOT `RequestMetadata::test_user` and NOT a fresh user per boot. Read lakekeeper's `RequestMetadata` constructors (`request_metadata.rs`) for the public non-test way to build an authenticated principal with a `UserId` (e.g. building from a parsed token actor); if none is public besides test_user, document the gap and use the minimal public path. (CE-1 finding: bootstrap needs `user_id()` present.)
- [ ] **Step 2:** `ensure_bootstrapped(state, system_user) -> Result<()>`: idempotent `bootstrap` with that principal (tolerate already-bootstrapped). Run lakekeeper migrations + this at server startup (Task 5 wires startup).
- [ ] **Step 3 (TDD):** a DB-gated test: `ensure_bootstrapped` twice → Ok both times; the default project exists afterward. Commit `feat(catalog-engine): bootstrap default project with a stable admin principal`.

### Task 4: provision a warehouse on workspace create
**Files:** `crates/egret-workspace/src/workspaces.rs` (the `create` hook) OR the workspace-create handler; `crates/egret-catalog-engine/src/provision.rs` (a reusable `provision_workspace_warehouse`).
- [ ] **Step 1:** `provision_workspace_warehouse(engine_state, egret_pool, workspace_id, slug, storage_cfg) -> Result<Uuid>`: idempotent (return existing from `workspace_warehouse` if present); else `create_warehouse` (name=slug, S3 profile = bucket from storage_cfg + key-prefix `workspaces/<slug>/warehouse`, MinIO endpoint+creds) using the system principal; on success `upsert_warehouse`. Reuse CE-1 `create_warehouse` call shape.
- [ ] **Step 2:** Rewire workspace creation: replace the A-2 `create_catalog` call (egret `catalogs` row) with `provision_workspace_warehouse`. Decide the cleanest seam — likely the workspace-create HTTP handler (which has app state incl. the engine + storage cfg), since `WorkspaceRepo` (egret-workspace crate) shouldn't depend on egret-catalog-engine. Keep it atomic-enough (workspace row committed, then warehouse provisioned; on warehouse failure, surface an error — a workspace without a warehouse is unusable).
- [ ] **Step 3 (TDD, DB+MinIO):** create a workspace via the path that provisions → `workspace_warehouse` row exists + the lakekeeper warehouse is resolvable (list it / `warehouse_for_slug`); idempotent. Commit `feat(workspace): provision a lakekeeper warehouse on workspace create`.

---

## Phase 2 — Mount + cutover

### Task 5: mount lakekeeper engine in egret-server; remove built-in router mount; preserve A-3 token ownership
**Files:** `crates/egret-server/src/lib.rs:94-99`; `crates/egret-catalog-engine/src/mount.rs` (front middleware + startup).
- [ ] **Step 1:** In egret-server startup: run lakekeeper migrations + `ensure_bootstrapped`, build the engine state (warehouse-backed; AllowAll), and mount `catalog_engine_router` nested at `/catalog/v1` with: (a) an egret `require_bearer` front layer that validates the bearer against `catalog_tokens` → workspace, resolves `warehouse_for_workspace`, and enforces the path `{prefix}` (warehouse UUID) `== that warehouse_id` (else 403; bad token 401) — porting the A-3 invariant but keyed on warehouse UUID; (b) the CE-1 `from_fn` `RequestMetadata` injection. REMOVE the `egret_catalog::iceberg_router(catalog)` mount (lib.rs:97). (lakekeeper is the only engine — no `catalog.engine` flag; the built-in code is deleted in Task 8.)
- [ ] **Step 2:** Build `cargo build --workspace`; the egret-catalog `iceberg_router`/`CatalogState` become unused by the server but still compile — that's fine until Task 8 deletes them. Ensure no compile breakage.
- [ ] **Step 3 (TDD, DB+MinIO):** an egret-server-level (or egret-catalog-engine) integration test driving the mounted `/catalog/v1` with egret tokens: provision two workspaces+warehouses+tokens; token A on A's prefix → 200 (create ns/table); token A on B's prefix → **403**; unknown token → **401**. Commit `feat(catalog-engine): mount lakekeeper engine, cut over from built-in router (A-3 ownership preserved)`.

---

## Phase 3 — Browse API reads lakekeeper

### Task 6: rewire browse API to lakekeeper data
**Files:** `crates/egret-catalog/src/browse.rs` (or a new `egret-catalog-engine` browse module the server mounts instead).
- [ ] **Step 1:** Re-point the browse reads from `CatalogStore` (egret `catalog_*`) to lakekeeper, scoped to the workspace's warehouse: resolve `warehouse_for_workspace(ctx.workspace_id)`, then read namespaces/tables/views/table-detail by calling lakekeeper's **in-process catalog Service** methods (the same `NamespaceService::list_namespaces` / `TablesService::*` the REST handlers use) with the system `RequestMetadata` + warehouse prefix — read `lakekeeper/crates/lakekeeper/src/api/iceberg/v1/{namespace,tables}.rs` for the Service method signatures. (Prefer the typed Service API over raw `lakekeeper` schema SQL, for rev-stability.) Keep `require_workspace(ctx, "catalog:read")` + workspace scoping. Build the same `CatalogSummary`/`TableDetail` DTOs the UI already consumes (unchanged shapes).
- [ ] **Step 2 (TDD, DB+MinIO):** a member with `catalog:read` lists namespaces/tables that were created via the engine (seed via the round-trip path); a non-member → 403; member-of-X requesting Y → 403 (reuse the A-3 IDOR test shape). Commit `feat(catalog-engine): browse API reads lakekeeper catalog data`.

---

## Phase 4 — Delete superseded A-1/A-2 (CE-8 folded in)

> Do this LAST: the engine serves (Task 5) + browse reads lakekeeper (Task 6), so the egret catalog is provably unused. Delete schema then code.

### Task 7: drop the catalog metadata tables (keep tokens + perms)
**Files:** new `migrations/0026_drop_legacy_catalog.sql` + touch `crates/egret-metastore/src/lib.rs`.
- [ ] **Step 1:** Migration: `DROP TABLE IF EXISTS` (CASCADE, child-before-parent or rely on CASCADE) the catalog metadata tables only — `catalog_table_partition_statistics, catalog_table_statistics, catalog_table_refs, catalog_table_metadata_log, catalog_table_snapshot_log, catalog_table_snapshots, catalog_table_properties, catalog_table_sort_orders, catalog_table_partition_specs, catalog_table_schemas, catalog_tables, catalog_view_properties, catalog_view_version_log, catalog_view_representations, catalog_view_versions, catalog_view_schemas, catalog_views, catalog_namespaces, catalogs`. **DO NOT drop `catalog_tokens`** (engine auth) or touch `role_permissions`/`roles`. Touch metastore lib.rs (re-embed); reset `egret_test`.
- [ ] **Step 2 (GATE):** fresh-DB migrate applies; `SELECT to_regclass('public.catalog_tokens')` non-null (kept), `to_regclass('public.catalog_tables')` NULL (dropped), `catalog:read` perm rows still present. Commit `feat(catalog): drop legacy catalog metadata tables (lakekeeper is the engine)`.

### Task 8: remove the superseded egret-catalog code
**Files:** `crates/egret-catalog/src/` (delete `service/{table_repo.rs,view_repo.rs,catalogs.rs}`, the table/view/namespace methods in `service/store.rs`, `api/*` Iceberg handlers, `resolve.rs`, `iceberg_router`+`CatalogState` from `routes.rs`); `crates/egret-server/src/lib.rs` (drop the `state.catalog` field + its mount); tests referencing the removed store.
- [ ] **Step 1:** Delete the modules/methods above. KEEP `service/tokens.rs` (catalog_tokens), `browse.rs` (now reads lakekeeper — Task 6), `require_bearer` (move it to where the engine front layer uses it, or keep exported), `CatalogError`, `BrowseState`. Remove the `catalog_*` store integration tests (store_tables/store_views/store_ns/store_catalogs/http_roundtrip) that exercise the deleted code; keep `store_tokens` + `browse_api`.
- [ ] **Step 2 (GATE):** `cargo build --workspace`; `cargo test -p egret-catalog -p egret-catalog-engine -p egret-server -p egret-workspace`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all --check`; `grep -c '^name = "iceberg"$' Cargo.lock` == 1; `grep -rn "iceberg_router\|table_repo\|view_repo" crates/egret-server crates/egret-catalog/src` returns nothing (gone). Commit `refactor(catalog): remove superseded egret catalog store/handlers (lakekeeper engine)`.

---

## Self-review checklist
- [ ] git-pinned deps; single iceberg; vendored `.sqlx`; test-utils in dev.
- [ ] `workspace_warehouse` provisioned on workspace create; idempotent; bootstrap stable across restarts.
- [ ] lakekeeper engine mounted; A-3 ownership (token A → B's prefix = 403) preserved at egret middleware.
- [ ] browse API + Catalog UI read lakekeeper; RBAC + cross-workspace 403 intact.
- [ ] superseded `catalog_*` schema + egret store/handlers DELETED; `catalog_tokens` + RBAC + browse + UI KEPT; suite green; clippy/fmt clean.

## Out of scope (later CE)
CE-3 (token→principal + per-token scope), CE-4 (real Authorizer = RBAC+scope, deny-before-sign), CE-5 (storage profiles/secrets, GCS/Azure), CE-6 (GC/task workers). **No CE-8 — its deletion is folded into Tasks 7-8 here.**
