# Control-Plane Polaris — Plan 1: remove the embedded-lakekeeper catalog Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.
> Spec: `docs/en/specs/2026-07-02-control-plane-polaris-catalog-design.md`. This is **P1 of the epic** (P1 remove → P2 storage/secrets to control plane → P3 `egret-polaris` client → P4 catalog+RBAC routes → P5 UI → P6 Polaris compose+e2e). Self-contained: greenfield removal; the workspace builds and tests green when done.
> VI reference: `docs/vi/plans/2026-07-02-polaris-p1-remove-embedded-lakekeeper.md`.

**Goal:** Delete egret's embedded-lakekeeper catalog (engine, token/warehouse identity layer, notebook catalog provisioning, kernel Daft-direct helpers, DB tables, workspace catalog UI) — greenfield, no migration — while **keeping** the storage-connection code (`egret-catalog::storage_conn`) untouched for P2.

**Architecture:** A compiler-guided removal: delete consumers first (notebook provisioning + kernel helpers, then the engine wiring + mint-token CLI), then the crates (`egret-catalog-engine`, `egret-iceberg-ext`) and the dead modules of `egret-catalog` (`tokens.rs`, `warehouse.rs`), then the DB tables and the workspace Catalog UI. `cargo build --workspace` + `codegraph_impact` are the safety nets.

**Tech Stack:** Rust (cargo workspace, sqlx, axum), Postgres migration, React/TS.

---

## Background the implementer needs

- **Crates & deps:** `egret-catalog-engine` (embedded lakekeeper + `front_auth` + `browse` + `provision_ws`), `egret-iceberg-ext` (legacy REST types), `egret-catalog` (`service/{tokens.rs, warehouse.rs, storage_conn.rs, store.rs}`). Cargo dependents of the catalog crates: `egret-bin`, `egret-kernel`, `egret-notebook`, `egret-server`, `egret-workspace` (+ the crates themselves). `egret-kernel` has **no** `egret_catalog*` usage (dep is removable). `egret-workspace`'s only reference is a doc-comment; it also uses `egret-catalog` storage (kept).
- **`egret-catalog` split:** REMOVE `tokens.rs` (catalog tokens) + `warehouse.rs` (`workspace_warehouses`). **KEEP** `storage_conn.rs` (storage connections — moved to control plane in P2), `error.rs`, `store.rs`, `mod.rs`, `lib.rs`.
- **Server wiring:** `egret-server/src/lib.rs` — `AppState.catalog_engine` + `catalog_browse` fields (lines ~33/41/64/65) and the `.nest("/catalog/v1", …)` + browse mount (lines ~116/129). `egret-bin/src/main.rs` — the catalog-engine assembly block (`run_lakekeeper_migrations`, `mount_catalog_engine`, `browse_router`, `ensure_namespace`, `catalog_engine`/`catalog_browse`/`catalog_engine_ctx`, ~lines 385–460 + wiring into `AppState`), the `catalog mint-token` CLI (`egret_catalog::service::tokens::Grant`, `fmt_scope`, ~lines 15/170–290), and the `EngineConnResolverImpl`/kernel wiring that references catalog (keep kernel engine-token; only remove catalog bits).
- **Notebook:** `egret-notebook/src/provisioning.rs` (`provision_notebook_access`, `Provisioned`, `NamespaceProvisioner`, uses `egret_catalog::service::tokens::Grant` + `CatalogStore`), `runtime.rs` (`build_notebook_runtime` calls `select_notebook_warehouse` + `provision_notebook_access`, returns manifest with `catalog: RuntimeCatalog`), `lib.rs:11` re-exports, `NotebookState.provisioner` + `.catalog` (`CatalogStore`) fields (routes.rs).
- **Kernel helpers:** `kernel-image/egret_kernel_helpers/{engines.py (iceberg/write_iceberg/_resolve_identifier), catalog.py, runtime.py (RuntimeCatalog)}`, `__init__.py`, `startup/00-egret.py`, `tests/{test_catalog.py, test_engines.py, test_runtime.py, test_init.py}`. Keep `trino`/`starrocks`/`write_table`.
- **Migrations:** repo-root `migrations/`; latest `0047` → next **`0048`**. After adding one, `touch crates/egret-metastore/src/lib.rs` (re-embed).
- **Frontend:** `frontend/src/App.tsx` routes `catalog` → `WarehousesList`/`WarehouseDetail` (remove); `storage`/`secrets` (keep until P2).

---

## Task 1: Remove the notebook's catalog dependency (kernel + backend)

**Files:**
- Modify: `kernel-image/egret_kernel_helpers/engines.py`, `__init__.py`, `startup/00-egret.py`, `runtime.py`
- Delete: `kernel-image/egret_kernel_helpers/catalog.py`, `kernel-image/tests/test_catalog.py`
- Modify: `kernel-image/tests/{test_engines.py, test_runtime.py, test_init.py}`
- Modify: `crates/egret-notebook/src/{runtime.rs, lib.rs, routes.rs}`; Delete: `crates/egret-notebook/src/provisioning.rs`

- [ ] **Step 1: Remove the Daft-direct kernel helpers**

In `engines.py` delete `iceberg()`, `write_iceberg()`, `_resolve_identifier()`. `git rm kernel-image/egret_kernel_helpers/catalog.py kernel-image/tests/test_catalog.py`. In `runtime.py` delete the `RuntimeCatalog` class + the `self.catalog = RuntimeCatalog(d["catalog"])` line. Fix `__init__.py` (`from .engines import starrocks, trino, write_table`; drop the `from .catalog import catalog` line; `__all__ = ["display","load_runtime","runtime","me","trino","starrocks","write_table"]`) and `startup/00-egret.py` (`from egret_kernel_helpers import display, runtime, me, trino, starrocks, write_table`). In `test_engines.py` delete the iceberg/write_iceberg tests + their `_fake_df`/`FakeCat` helpers; in `test_runtime.py` drop `catalog` assertions; in `test_init.py` drop `catalog`/`iceberg`/`write_iceberg` from expected exports.

- [ ] **Step 2: Run the kernel tests**

Run: `cd kernel-image && python -m pytest tests/ -q`
Expected: PASS, no import errors.

- [ ] **Step 3: Gut the backend notebook provisioning**

`git rm crates/egret-notebook/src/provisioning.rs`. In `runtime.rs`: remove `use crate::provisioning::provision_notebook_access;`, the `list_warehouses`+`select_notebook_warehouse` block, the `provision_notebook_access(...)` call, `select_notebook_warehouse` + its tests + `NOTEBOOK_WAREHOUSE_*` const, and the `catalog: RuntimeCatalog {…}` field from the returned manifest + the `RuntimeCatalog` struct. In `lib.rs` remove the `pub use provisioning::…` line. In `routes.rs` remove `NotebookState.provisioner` + `NotebookState.catalog` (`CatalogStore`) fields (and the `NotebookRuntime.catalog` field). Keep `username`, `engines`, `catalogs`, `engine_jwt`.

- [ ] **Step 4: Build + test the notebook crate; fix the bin wiring it breaks**

Run: `cargo build -p egret-notebook`
Expected: builds. Then fix `egret-bin` where it constructs `NotebookState { provisioner, catalog, … }` — remove those two fields from the construction (the compiler points to the exact site). Re-run `cargo build -p egret-bin -p egret-notebook`.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor(notebook): remove Daft-direct catalog helpers + catalog provisioning (engine-only)"
```

---

## Task 2: Remove the catalog-engine wiring + mint-token CLI

**Files:**
- Modify: `crates/egret-server/src/lib.rs`, `crates/egret-bin/src/main.rs`, `crates/egret-bin/src/cli.rs`

- [ ] **Step 1: Remove the AppState catalog fields + nesting**

In `egret-server/src/lib.rs`: delete the `catalog_engine: Option<axum::Router>` and `catalog_browse: Option<axum::Router>` fields, their `None` initializers in `without_db()`, and the two mount blocks (`if let Some(catalog_engine) … .nest("/catalog/v1", …)` and the `catalog_browse` mount).

- [ ] **Step 2: Remove the catalog-engine assembly + mint-token CLI in egret-bin**

In `egret-bin/src/main.rs`: delete the entire catalog-engine assembly block (the `let (catalog_engine, catalog_browse, catalog_engine_ctx) = if config.catalog.enabled { … }` region with `run_lakekeeper_migrations`, `build_state`, `ensure_bootstrapped`, `mount_catalog_engine`, `browse_router`, `spawn_gc_workers`, `ensure_namespace`), the `catalog_base_url` hoist, and the `catalog_engine`/`catalog_browse` fields passed into `AppState`. Delete the `catalog mint-token` command handler + `fmt_scope` + the `use egret_catalog::service::CatalogStore;`/`tokens::Grant` imports. In `cli.rs` remove the `Catalog`/`MintToken` subcommand + its parse tests.

- [ ] **Step 3: Build egret-bin**

Run: `cargo build -p egret-bin`
Expected: builds (compiler flags any remaining `egret_catalog_engine`/token references — remove them). `EngineConnResolverImpl` + kernel engine-token wiring stay.

- [ ] **Step 4: Commit**

```bash
git add crates/egret-server crates/egret-bin
git commit -m "refactor(catalog): remove catalog-engine mount + mint-token CLI"
```

---

## Task 3: Delete the `egret-catalog-engine` and `egret-iceberg-ext` crates

**Files:**
- Delete: `crates/egret-catalog-engine/`, `crates/egret-iceberg-ext/`
- Modify: root `Cargo.toml` (workspace members + `[patch]`), dependents' `Cargo.toml`

- [ ] **Step 1: Confirm nothing live still references them**

Run: `rg "egret_catalog_engine|egret-catalog-engine|egret_iceberg_ext|egret-iceberg-ext|lakekeeper" crates --files-with-matches` and `codegraph_impact` on any surviving symbol. Expected: only the two crates themselves + Cargo files (after Tasks 1–2).

- [ ] **Step 2: Remove the crates + workspace/deps wiring**

`git rm -r crates/egret-catalog-engine crates/egret-iceberg-ext`. In root `Cargo.toml`: remove both from `[workspace] members` and any `[workspace.dependencies]` entries, and remove the `[patch."https://github.com/lakekeeper/lakekeeper"]` block (if present). Remove `egret-catalog-engine`/`egret-iceberg-ext` from `egret-bin`, `egret-server`, `egret-workspace`, `egret-notebook` `Cargo.toml` `[dependencies]` wherever present.

- [ ] **Step 3: Build the workspace**

Run: `cargo build --workspace`
Expected: builds. Fix any remaining dependent references the compiler surfaces.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "chore(catalog): delete egret-catalog-engine + egret-iceberg-ext (embedded lakekeeper removed)"
```

---

## Task 4: Gut `egret-catalog` — drop tokens + warehouse, keep storage

**Files:**
- Delete: `crates/egret-catalog/src/service/tokens.rs`, `crates/egret-catalog/src/service/warehouse.rs`, their tests under `crates/egret-catalog/tests/`
- Modify: `crates/egret-catalog/src/service/mod.rs`, `lib.rs`

- [ ] **Step 1: Remove the token + warehouse modules**

`git rm crates/egret-catalog/src/service/tokens.rs crates/egret-catalog/src/service/warehouse.rs`. Delete their test files (`rg -l "insert_warehouse|authenticate_token|mint" crates/egret-catalog/tests` → `git rm` those). In `service/mod.rs` remove `pub mod tokens;` / `pub mod warehouse;` and any re-exports; in `lib.rs` remove re-exports of token/warehouse types. Keep `storage_conn`, `store`, `error`.

- [ ] **Step 2: Build egret-catalog + dependents**

Run: `cargo build -p egret-catalog`
Expected: builds (only storage + store + error remain). Then `cargo build --workspace`; fix `egret-workspace`/`egret-bin` if they referenced token/warehouse types (they should not — storage stays).

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "refactor(catalog): drop catalog tokens + warehouse modules (keep storage connections)"
```

---

## Task 5: Drop the catalog DB tables

**Files:**
- Create: `migrations/0048_drop_embedded_catalog.sql`
- Modify: `crates/egret-metastore/src/lib.rs` (touch)

- [ ] **Step 1: Write the migration**

```sql
-- Control-plane Polaris pivot: remove the embedded-lakekeeper catalog storage.
-- Greenfield — no migration of catalog data.
DROP TABLE IF EXISTS catalog_tokens CASCADE;
DROP TABLE IF EXISTS workspace_warehouses CASCADE;
DROP SCHEMA IF EXISTS lakekeeper CASCADE;
```

- [ ] **Step 2: Re-embed + apply**

Run: `touch crates/egret-metastore/src/lib.rs && cargo run -q -p egret-bin -- migrate`
Then: `docker exec egret-dev-postgres-1 psql -U egret -d egret -c "\dt catalog_tokens; \dn lakekeeper"`
Expected: `catalog_tokens`/`workspace_warehouses` gone; no `lakekeeper` schema.

- [ ] **Step 3: Commit**

```bash
git add migrations/0048_drop_embedded_catalog.sql crates/egret-metastore/src/lib.rs
git commit -m "feat(db): drop catalog_tokens, workspace_warehouses, lakekeeper schema"
```

---

## Task 6: Remove the workspace Catalog UI

**Files:**
- Modify: `frontend/src/App.tsx`; Delete: `frontend/src/routes/explore/catalog/`

- [ ] **Step 1: Remove the routes + pages**

In `App.tsx` remove the `catalog`/`catalog/:wid` routes + the `WarehousesList`/`WarehouseDetail` imports, and the "Catalog" nav entry (search the sidebar/nav component for `/catalog`). `git rm -r frontend/src/routes/explore/catalog`. (Leave `storage`/`secrets` routes — moved in P2.) Remove the catalog API client module if unused (`rg` it).

- [ ] **Step 2: Typecheck + build**

Run: `cd frontend && npm run typecheck && npm run build`
Expected: clean (no dangling imports).

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "refactor(ui): remove workspace Catalog pages (catalog moves to control plane)"
```

---

## Task 7: Workspace verification

- [ ] **Step 1: Full backend check**

Run:
```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test --workspace
```
Expected: all clean/pass. No dead-code/unused-dep warnings from the removals (fix any: unused deps in Cargo.toml).

- [ ] **Step 2: Frontend + kernel**

Run: `cd frontend && npm run typecheck && npm run lint && npm run build` and `cd kernel-image && python -m pytest tests/ -q`
Expected: clean.

- [ ] **Step 3: Confirm the catalog surface is gone**

Run: `rg "catalog/v1|mount_catalog_engine|catalog_tokens|workspace_warehouses|egret_catalog_engine" crates` → only comments/none. `curl -s -o /dev/null -w "%{http_code}" http://localhost:8080/catalog/v1/config` after `serve` → connection refused / 404 (route gone).

---

## Self-review notes (spec coverage for P1)

- Spec **A. Removal (greenfield)**: Tasks 1–6 (kernel+notebook helpers, engine wiring+mint-token, crate deletion, egret-catalog gutting, DB drop, workspace Catalog UI).
- **Kept for P2:** `egret-catalog::storage_conn` + the workspace Storage/Secrets pages (moved, not removed, in P2).
- **Out of P1:** storage/secrets → control plane (P2), `egret-polaris` (P3), catalog+RBAC routes (P4), UI (P5), Polaris compose/e2e (P6).
- Removal is compiler-guided: each deletion is followed by `cargo build` to surface dependents; `rg`/`codegraph_impact` confirm no live references before deleting a crate.
