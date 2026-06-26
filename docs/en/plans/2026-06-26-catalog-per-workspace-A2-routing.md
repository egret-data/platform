# Catalog-per-workspace — Plan A-2: Prefix routing + per-request resolution

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make the Iceberg REST catalog resolve a workspace's catalog per-request from a `{prefix}` path segment (workspace slug), removing the TEMP single-default-catalog bootstrap from A-1; provision a catalog automatically when a workspace is created. Also fix 3 review findings carried over from A-1.

**Architecture:** Add `/v1/{prefix}/...` to the Iceberg REST router (prefix = workspace slug). A request-scoped resolver turns `{prefix}` into a `ResolvedCatalog { catalog_id, warehouse }` (404 if the workspace/catalog doesn't exist) and hands it to the handlers, which stop reading the process-global `CatalogService.catalog_id` / `CatalogState.warehouse`. `/v1/config?warehouse=<slug>` advertises `overrides.prefix` so Daft/Trino auto-scope. Workspace creation provisions the `catalogs` row.

**Tech Stack:** Rust, axum 0.8, sqlx (Postgres), iceberg-rust fork `bb70173`.

**Base:** branch from `main` (A-1 is merged at `15d30fa`). DB-gated tests: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`.

**Scope:** This is plan 2 of 3 for spec A. Token-based engine auth (`catalog_tokens` in `require_bearer`), RBAC `catalog:*` perms, the internal UI API, and the frontend Catalog view are Plan A-3 — NOT here. A-2 keeps the existing static bearer token; it only adds prefix resolution + provisioning.

**Spec:** `docs/superpowers/specs/2026-06-26-catalog-per-workspace-design.md` (§2.1, §2.2, §4).

---

## File map
- Modify: `crates/egret-catalog/src/service/store.rs` — `drop_namespace` (finding #1), `cas_commit` (finding #3).
- Modify: `crates/egret-catalog/src/service/view_repo.rs` — `clear_view_metadata` delete order (finding #2).
- Modify: `crates/egret-catalog/src/service/catalogs.rs` — add `get_catalog_by_slug` (join workspaces).
- Modify: `crates/egret-catalog/src/routes.rs` — add `{prefix}` to every route; add the resolver layer.
- Create: `crates/egret-catalog/src/api/resolve.rs` — `ResolvedCatalog` extractor/middleware.
- Modify: `crates/egret-catalog/src/api/{namespaces,tables,views,config,sign}.rs` — use the resolved catalog instead of `s.service.catalog_id` / `s.warehouse`.
- Modify: `crates/egret-catalog/src/service/service.rs` — drop `catalog_id` (and `warehouse` if now unused) from `CatalogService`.
- Modify: `crates/egret-bin/src/main.rs` — remove `ensure_default_catalog` call + the `catalog_id` field; keep storage/base_url.
- Modify: `crates/egret-catalog/src/service/catalogs.rs` — remove `ensure_default_catalog` (now dead).
- Modify: `crates/egret-workspace/src/workspaces.rs` (or its route handler) — provision a `catalogs` row on workspace create.
- Modify: `crates/egret-catalog/tests/http_roundtrip.rs` — exercise the prefixed URLs.

---

## Phase 0 — Carry-over fixes from the A-1 review (small, independently testable)

### Task 1: `drop_namespace` must count views too (finding #1)

**Files:** `crates/egret-catalog/src/service/store.rs`; test `crates/egret-catalog/tests/store_ns.rs`

- [ ] **Step 1 — failing test.** In `store_ns.rs`, add `drop_namespace_with_only_a_view_is_not_empty`: seed catalog+namespace, insert a view (use `test_support::minimal_view_metadata`), then assert `s.drop_namespace(cid, &levels)` returns `Err(CatalogError::NamespaceNotEmpty(_))` (NOT a `Db` error).
- [ ] **Step 2 — run, expect FAIL** (`cargo test -p egret-catalog --test store_ns drop_namespace_with_only_a_view_is_not_empty`): currently the namespace is reported empty and the DELETE FK-violates → `CatalogError::Db`.
- [ ] **Step 3 — fix.** In `drop_namespace`, replace the tables-only count with a combined check:
```rust
let count: i64 = sqlx::query_scalar::<_, i64>(
    "SELECT (SELECT count(*) FROM catalog_tables WHERE namespace_id = $1) \
          + (SELECT count(*) FROM catalog_views  WHERE namespace_id = $1)",
)
.bind(ns.id).fetch_one(&self.db.pool).await.map_err(CatalogError::Db)?;
```
- [ ] **Step 4 — run, expect PASS** (the new test + existing store_ns tests).
- [ ] **Step 5 — commit:** `fix(catalog): drop_namespace also counts views (was 500 not 409)`.

### Task 2: `clear_view_metadata` correct delete order (finding #2)

**Files:** `crates/egret-catalog/src/service/view_repo.rs`; test `crates/egret-catalog/tests/store_views.rs`

- [ ] **Step 1 — failing test.** Add `clear_view_metadata_removes_all_children`: insert a view, open a tx, call `view_repo::clear_view_metadata(&mut tx, view_id)`, commit, then assert zero rows remain in `catalog_view_schemas/_versions/_representations/_version_log/_properties` for that `view_id` AND that `clear_view_metadata` returned `Ok` (today it errors on the FK). (Expose a small test seam if needed, or test via a future `update_view`; simplest is a direct `view_repo` call in an integration test.)
- [ ] **Step 2 — run, expect FAIL** (the `DELETE FROM catalog_view_schemas` FK-violates because `catalog_view_versions.(view_id,schema_id)` has no `ON DELETE CASCADE`).
- [ ] **Step 3 — fix.** Reorder deletes in `clear_view_metadata` to honor FKs (children before parents): representations → version_log → versions → schemas → properties:
```rust
// catalog_view_representations FK -> versions (cascades, but delete explicitly for clarity/safety)
// catalog_view_version_log FK -> views only
// catalog_view_versions FK -> schemas (NO cascade) — must delete BEFORE schemas
for tbl in ["catalog_view_representations","catalog_view_version_log","catalog_view_versions","catalog_view_schemas","catalog_view_properties"] {
    sqlx::query(&format!("DELETE FROM {tbl} WHERE view_id = $1")).bind(view_id).execute(&mut **tx).await.map_err(CatalogError::Db)?;
}
```
(Keep it as explicit statements if you prefer not to format table names.) Correct the doc comment to state the real order/no-cascade.
- [ ] **Step 4 — run, expect PASS.**
- [ ] **Step 5 — commit:** `fix(catalog): clear_view_metadata deletes children in FK order`.

### Task 3: `cas_commit` must persist `format_version` (finding #3)

**Files:** `crates/egret-catalog/src/service/store.rs`; test `crates/egret-catalog/tests/store_tables.rs`

- [ ] **Step 1 — failing test.** Add `cas_commit_persists_format_version`: insert a table, then `cas_commit` with metadata whose `format_version()` you can assert is re-read correctly after `load_table`. (Since egret always builds V2, assert the round-trip keeps V2 AND that the UPDATE sets the column — if a pure-V2 assertion can't fail, at minimum assert the SET clause includes `format_version` by reloading and checking `row.metadata.format_version()`; add a comment that the real trigger is register+UpgradeFormatVersion.)
- [ ] **Step 2 — run.** (May pass trivially if V2-only; the value is the regression guard + the column write.)
- [ ] **Step 3 — fix.** Add `format_version = $N` to the `cas_commit` UPDATE SET clause, binding `table_repo::format_version_to_i16(metadata.format_version())` (mirror `insert_table`). Renumber binds.
- [ ] **Step 4 — run, expect PASS** (all store_tables tests).
- [ ] **Step 5 — commit:** `fix(catalog): cas_commit persists format_version`.

---

## Phase 1 — Per-workspace catalog resolution

### Task 4: `get_catalog_by_slug`

**Files:** `crates/egret-catalog/src/service/catalogs.rs`; test `store_catalogs.rs`

- [ ] Add `pub async fn get_catalog_by_slug(&self, slug: &str) -> Result<Catalog, CatalogError>` joining `catalogs` to `workspaces`:
```sql
SELECT c.id, c.workspace_id, c.name, c.warehouse_location
FROM catalogs c JOIN workspaces w ON w.id = c.workspace_id WHERE w.slug = $1
```
NotFound if absent. TDD: test that a seeded `(workspace slug, catalog)` resolves; an unknown slug → NotFound. Commit `feat(catalog): resolve catalog by workspace slug`.

### Task 5: Prefix routing + `ResolvedCatalog`

**Files:** `crates/egret-catalog/src/routes.rs`, new `src/api/resolve.rs`, all `src/api/*.rs` handlers.

- [ ] **Routes:** change every Iceberg route in `iceberg_router` to carry a leading `/{prefix}` segment, e.g. `/v1/{prefix}/namespaces`, `/v1/{prefix}/namespaces/{ns}/tables/{table}`, `/v1/{prefix}/aws/s3/sign`. Leave `/v1/config` WITHOUT prefix (it's the discovery endpoint; it takes `?warehouse=` instead — Task 6).
- [ ] **Resolver:** in `resolve.rs`, define `pub struct ResolvedCatalog { pub catalog_id: Uuid, pub warehouse: String }` and an axum extractor (e.g. `FromRequestParts`) that reads the `prefix` path param and calls `store.get_catalog_by_slug(prefix)` → `ResolvedCatalog`, returning `CatalogError::NotFound` (404) if absent. The extractor needs `CatalogState` (pull `store` from it). Handlers add `cat: ResolvedCatalog` as an argument.
- [ ] **Handlers:** replace `s.service.catalog_id` with `cat.catalog_id` and `&s.warehouse` (in `api/tables.rs`/`api/views.rs` for `table_base_location`) with `&cat.warehouse`. Update `service.rs` `create_table`/`load_table`/`commit_table` to take a `catalog_id` argument (instead of reading `self.catalog_id`) — or have handlers call the store directly with `cat.catalog_id`. Pick the smaller diff; keep `CatalogService` as a thin holder of `store` + `storage`.
- [ ] **Verify:** the existing `http_roundtrip` test will break (URLs lack prefix) — it's updated in Task 9; for now confirm `cargo build -p egret-catalog` compiles and `cargo test -p egret-catalog --test store_ns --test store_tables --test store_views --test store_catalogs` stay green (store-level tests don't touch routing).
- [ ] Commit `feat(catalog): per-request {prefix} catalog resolution`.

### Task 6: prefix-aware `/v1/config`

**Files:** `crates/egret-catalog/src/api/config.rs`

- [ ] `get_config` reads an optional `?warehouse=<slug>` query param. When present, resolve the catalog by slug and return `overrides` containing `prefix = <slug>` and `warehouse = <catalog.warehouse_location>` (plus the existing s3 signer keys). When absent, keep returning the signer keys without a prefix (back-compat for discovery). TDD via `http_roundtrip` (Task 9) or a focused config test. Commit `feat(catalog): /v1/config advertises per-workspace prefix`.

### Task 7: remove the TEMP bootstrap

**Files:** `crates/egret-bin/src/main.rs`, `crates/egret-catalog/src/service/{service.rs,catalogs.rs}`

- [ ] Delete the `ensure_default_catalog` call + the `catalog_id` field from the `CatalogService` construction in `main.rs`. Remove `catalog_id` from the `CatalogService` struct (and `warehouse` from `CatalogService`/`CatalogState` if nothing reads it anymore — the per-request `cat.warehouse` replaces it; keep `CatalogState.warehouse` only if `/v1/config` without `?warehouse` still needs a fallback — decide and note). Remove `ensure_default_catalog` from `catalogs.rs`. Ensure `cargo build --workspace` is clean. Commit `refactor(catalog): drop TEMP single-default-catalog bootstrap`.

---

## Phase 2 — Provisioning

### Task 8: provision a catalog on workspace create

**Files:** `crates/egret-workspace/src/workspaces.rs` (or the create route handler), `crates/egret-workspace/Cargo.toml` if a dep is needed.

- [ ] When a workspace is created (`WorkspaceRepo::create`, workspaces.rs:19), create its `catalogs` row with `warehouse_location` derived from the slug, e.g. `s3://<bucket>/workspaces/<slug>/warehouse`. AVOID a circular crate dependency: if `egret-workspace` can't depend on `egret-catalog`, do the provisioning in the workspace **HTTP create handler** (egret-workspace `routes.rs`) or in `egret-server`/`egret-bin` wiring where both stores are available, OR insert the `catalogs` row directly via SQL in `WorkspaceRepo::create` within the same tx (simplest, no cross-crate dep — the `catalogs` table is in the shared DB). Make it idempotent (one catalog per workspace; ignore AlreadyExists). The bucket comes from storage config — thread it in or read from env/config. TDD: creating a workspace yields a resolvable catalog (`get_catalog_by_slug` succeeds). Commit `feat(workspace): provision an Iceberg catalog on workspace create`.

---

## Phase 3 — Integration

### Task 9: prefixed `http_roundtrip` + manual Trino note

**Files:** `crates/egret-catalog/tests/http_roundtrip.rs`; docs.

- [ ] Update `http_roundtrip` `state()` + requests to seed a workspace+catalog and hit the prefixed URLs (`/v1/{slug}/namespaces/...`), and assert `/v1/config?warehouse=<slug>` returns `overrides.prefix == <slug>`. Add a case: a request to an UNKNOWN prefix → 404. Re-enable the table create/load assertions through the prefixed routes.
- [ ] Run the whole suite green: `cargo test -p egret-catalog`, `cargo clippy -p egret-catalog -p egret-bin --all-targets -- -D warnings`, `cargo fmt --all --check`.
- [ ] Update the manual Trino integration note (memory `egret-manual-trino-integration-test`) to use the prefixed catalog URL + the workspace slug as warehouse. Commit `test(catalog): prefixed REST round-trip + config prefix`.

---

## Self-review checklist
- [ ] Findings #1/#2/#3 each have a regression test.
- [ ] Every Iceberg route carries `{prefix}`; `/v1/config` resolves `?warehouse`.
- [ ] No handler reads a process-global `catalog_id`/`warehouse` anymore; all go through `ResolvedCatalog`.
- [ ] `ensure_default_catalog` and `CatalogService.catalog_id` are gone.
- [ ] Workspace create provisions a catalog; unknown prefix → 404; cross-workspace prefix can't reach another workspace's tables.
- [ ] `cargo test -p egret-catalog` green; clippy + fmt clean; `cargo build --workspace` clean.

## Out of scope (Plan A-3)
- `catalog_tokens` auth in `require_bearer`; RBAC `catalog:read|write`.
- Internal UI API + frontend Catalog view.
- Per-workspace MinIO IAM credential isolation.
