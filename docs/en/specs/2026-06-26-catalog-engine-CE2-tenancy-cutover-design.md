# Catalog Engine CE-2 — Tenancy + provisioning + cutover — Design

**Date:** 2026-06-26
**Status:** Design — pending review
**Branch:** `feat/catalog-engine-lakekeeper` (builds on CE-1 spike)
**Spec parent:** `docs/en/specs/2026-06-26-catalog-engine-lakekeeper-embed-design.md`
**Findings input:** `docs/en/specs/2026-06-26-catalog-engine-CE1-findings.md`

## 0. Goal

Turn the CE-1 spike into a real, served, per-workspace lakekeeper catalog: clean up the spike shortcuts, provision a lakekeeper warehouse per egret workspace, mount lakekeeper's Iceberg REST router into egret-server **as the catalog engine (replacing the built-in A-1/A-2 `iceberg_router`)**, and rewire the internal browse API + Catalog UI to read lakekeeper data so they keep working after the cutover. Per-workspace token security (A-3) is preserved at the egret middleware layer; real per-token authz scope is deferred to CE-3/CE-4.

## 1. Decisions (locked)
- **Dependency:** git-pin lakekeeper to an **upstream rev** (`github.com/lakekeeper/lakekeeper`), replacing the CE-1 sibling path-deps. Reproducible; no sibling checkout needed.
- **Tenancy mapping:** a **new `workspace_warehouse` table** (not the A-1 `catalogs` table, which CE-8 retires).
- **Cutover:** **replace** the built-in `iceberg_router` mount immediately — only lakekeeper serves Iceberg REST. (egret's `catalog_*` schema/store stays dormant until CE-8.)
- **UI continuity:** CE-2 **pulls the read half of CE-7 forward** — the internal browse API reads lakekeeper, so the Catalog UI doesn't break.
- Tenancy model: one lakekeeper **default project**; **one warehouse per workspace**; REST `{prefix}` = warehouse **UUID**.

## 2. Components

### 2.1 Dependencies (git-pin) + spike-shortcut cleanup
- Replace `path = "../../../lakekeeper/..."` with `git = "https://github.com/lakekeeper/lakekeeper", rev = "<pinned>"` for `lakekeeper` (feature `router`), `lakekeeper-storage-postgres`, `lakekeeper-io`. Choose a rev that matches the iceberg fork `bb70173` (verify single `iceberg` still resolves).
- **SQLX_OFFLINE:** the `.sqlx` cache is inside lakekeeper's source. With a git dep, point `SQLX_OFFLINE_DIR` at the fetched git checkout under `~/.cargo/git/checkouts/...` is NOT stable. Resolve by **vendoring lakekeeper's `.sqlx` query cache into egret** (commit a copy under e.g. `crates/egret-catalog-engine/.sqlx-lakekeeper/`) and pointing `SQLX_OFFLINE_DIR` there, OR regenerate egret-catalog-engine's own `.sqlx`. CE-2 settles this concretely.
- Move the lakekeeper `test-utils` feature from `[dependencies]` to `[dev-dependencies]`; remove the `test_user` bootstrap path from production code (replaced by §2.2).
- Keep the `tokio_unstable`/`tracing_unstable` rustflags (`.cargo/config.toml`); document for CI.

### 2.2 Startup bootstrap (real admin principal)
At egret-server startup (when `catalog.engine = lakekeeper`): run lakekeeper migrations (schema `lakekeeper`), then `bootstrap` the default project using a **real admin principal** — a `RequestMetadata` carrying a stable `UserId` derived from an egret system/superadmin identity (config: `catalog.system_user`), NOT `test_user`. Idempotent (skip if already bootstrapped). This addresses the CE-1 `MissingUserId` finding.

### 2.3 Provisioning + `workspace_warehouse`
- New migration: `workspace_warehouse(workspace_id uuid PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE, warehouse_id uuid NOT NULL, created_at timestamptz default now())`.
- On workspace create (rewire the A-2 hook in `WorkspaceRepo::create` / its provisioning seam): call lakekeeper `create_warehouse` (name = workspace slug; `StorageProfile::S3` from egret storage config: bucket + key-prefix `workspaces/<slug>/warehouse`, MinIO/S3 endpoint, credentials) + store the returned `warehouse_id` in `workspace_warehouse`. **Idempotency:** look up `workspace_warehouse` first; if present, skip. (Per CE-1: do NOT sniff `create_warehouse` errors.)
- Resolution: `slug → workspace_id → warehouse_id` (join `workspaces` + `workspace_warehouse`). egret hands engines the `{prefix}` (=warehouse_id) via `/catalog/v1/config?warehouse=<slug>` if egret fronts config, or engines discover it from lakekeeper's config response.

### 2.4 Mount + cutover
- New `egret-catalog-engine::mount_into(router, state, dsn)` (built on CE-1's `catalog_engine_router`): assemble lakekeeper `State` (real warehouse-backed; AllowAll authorizer for now) and nest `new_v1_full_router` under `/catalog/v1` in egret-server.
- **Front middleware (preserve A-3 security):** egret `require_bearer` runs first — validate the bearer against `catalog_tokens` → workspace; resolve that workspace's `warehouse_id`; enforce `{prefix} == warehouse_id` (the A-3 per-workspace ownership invariant, now keyed on warehouse UUID; 403 on mismatch, 401 on bad token). Then a `from_fn` layer injects a lakekeeper `RequestMetadata` (the CE-1 seam). Inside lakekeeper: AllowAll for CE-2 (real per-token scope = CE-3/CE-4).
- **Remove** the built-in `iceberg_router` mount from egret-server (the A-1/A-2 Iceberg REST surface). Leave egret's `catalog_*` schema + `egret-catalog` store dormant (unused) — CE-8 deletes them. Guard the engine choice behind `catalog.engine` config; default `lakekeeper` on this branch.

### 2.5 Browse API rewire (read half of CE-7)
- Re-point the internal `/api/ws/{ws}/catalog/...` endpoints (namespaces / tables / views / table-detail / summary) to read **lakekeeper** data instead of egret `catalog_*`: resolve the workspace's `warehouse_id`, then read via lakekeeper's catalog read API (or query the `lakekeeper` schema for namespaces/tables under that warehouse). Keep the `require_workspace(ctx, "catalog:read")` RBAC gate + workspace scoping (a member of X can't read Y). The Catalog UI is unchanged (same internal API shape).

## 3. Testing
- **Provisioning:** create a workspace → `workspace_warehouse` row exists + the lakekeeper warehouse is resolvable; idempotent on repeat.
- **Per-workspace REST round-trip:** through the mounted lakekeeper router with egret token auth — token for workspace A on A's `{prefix}` → 200 create/load; token A on workspace B's `{prefix}` → **403** (egret middleware); unknown token → 401.
- **Browse API:** lists namespaces/tables from lakekeeper for a member with `catalog:read`; non-member → 403; cross-workspace → 403.
- **Cutover:** the built-in `iceberg_router` is gone; whole `cargo test` (catalog-engine + server + workspace) green; clippy/fmt clean; single `iceberg` version retained.

## 4. Risks / notes
- **Bootstrap principal model:** mapping an egret system identity to a lakekeeper user must be stable across restarts (don't create a new user each boot). Settle the `catalog.system_user` shape.
- **Browse-read against lakekeeper:** prefer lakekeeper's typed read API over raw `lakekeeper` schema queries (schema may change across lakekeeper revs); if no clean read API exists, the raw query is acceptable but pinned to the lakekeeper rev.
- **SQLX_OFFLINE with a git dep** — §2.1 must produce a reproducible answer (vendor the cache).
- **egret `catalog_*` dormant** — ensure nothing still writes/reads it post-cutover except the browse API (which §2.5 rewires away). Grep to confirm before merge.

## 5. Out of scope (later CE)
CE-3 (token→principal injection + per-token scope), CE-4 (real `Authorizer` = RBAC + scope, deny-before-sign), CE-5 (storage profiles/secrets bridge, GCS/Azure), CE-6 (GC/task workers), CE-8 (delete egret `catalog_*` schema + `egret-catalog` store/handlers).
