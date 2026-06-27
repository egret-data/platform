# Catalog Engine CE-1 — Spike + Foundation (embedded lakekeeper) Plan

> **For agentic workers:** This is a DISCOVERY SPIKE, not a pre-scripted TDD plan. Execute it as a single focused effort by one capable agent that iterates and produces (a) a working vertical slice and (b) a FINDINGS report that informs the CE-2..8 plans. Hard acceptance gates below are the definition of done. Steps use `- [ ]`.

**Goal:** Prove egret can embed lakekeeper's core as a library — build it (shared iceberg rev), run its migrator in an isolated Postgres schema, mount its Iceberg REST router in-process under `/catalog` with `AllowAllAuthorizer`, and round-trip a real catalog op through lakekeeper's store.

**Architecture:** New `egret-catalog-engine` crate owns all lakekeeper wiring. lakekeeper runs against a dedicated pool with `search_path=lakekeeper` (isolating its 63 migrations + `_sqlx_migrations` from egret's `public`). Router built via `lakekeeper::api::iceberg::new_v1_full_router`, state = lakekeeper-storage-postgres `CatalogState` + `AllowAllAuthorizer` + postgres secrets + one S3 storage profile. Ported from `lakekeeper-bin/src/serve.rs`.

**Tech Stack:** Rust, axum, sqlx (Postgres), `lakekeeper` + `lakekeeper-storage-postgres` + `lakekeeper-io` crates (git-pinned, same `iceberg-rust` fork rev `bb70173` as egret), MinIO/S3.

**Reference files (port from these — read first):**
- `data-portal/lakekeeper/crates/lakekeeper-bin/src/serve.rs` — the canonical embed wiring (pools, `CatalogState::from_pools`, router build, `modify_router_fn`).
- `data-portal/lakekeeper/crates/lakekeeper-bin/src/main.rs` — migrate + bootstrap + warehouse provisioning sequence.
- `data-portal/lakekeeper/crates/lakekeeper/src/api/iceberg/mod.rs:46` — `new_v1_full_router` signature + its trait bounds (the state `S` must satisfy these).
- `data-portal/lakekeeper/crates/lakekeeper-storage-postgres/src/lib.rs` — `CatalogState`, `build_connect_ops`, the migrations module.
- `data-portal/lakekeeper/crates/lakekeeper-bin/src/authorizer.rs` — `AllowAllAuthorizer` usage.

**Spec:** `docs/superpowers/specs/2026-06-26-catalog-engine-lakekeeper-embed-design.md`

**Prereqs:** branch `feat/catalog-engine-lakekeeper` (already on it). DB: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`. MinIO is up (`egret-dev-minio-1`, :9000).

---

## Task 1: Dependencies build with a single `iceberg` version

**Files:** `Cargo.toml` (workspace), new `crates/egret-catalog-engine/Cargo.toml` + `src/lib.rs`.

- [ ] **Step 1:** Decide the lakekeeper dep source. Default: **git dep pinned to a rev** of `github.com/lakekeeper/lakekeeper` (reproducible). If co-developing against the local sibling checkout, a `path = "../../lakekeeper/crates/lakekeeper"` dep is acceptable for the spike — RECORD which you used. Add `lakekeeper` (features `["router"]`), `lakekeeper-storage-postgres`, `lakekeeper-io` to the workspace deps.
- [ ] **Step 2:** Create `crates/egret-catalog-engine` (a new workspace member) that depends on the three lakekeeper crates + egret-config + egret-metastore. Empty `lib.rs` for now.
- [ ] **Step 3 (GATE):** `cargo build -p egret-catalog-engine 2>&1 | tail` → builds. Then `cargo tree -d -p egret-catalog-engine | grep -A2 '^iceberg'` → **exactly one `iceberg` version** (the `bb70173` fork). If two appear, align egret's pin to lakekeeper's (they're the same rev today, so this should pass) — this is the make-or-break gate.
- [ ] **Step 4:** Commit `chore(catalog-engine): add lakekeeper as a dependency`.

**If two iceberg versions appear:** STOP and report — the embed is blocked until the pins align (vendor or bump). This is the #1 risk; settle it before any further work.

---

## Task 2: lakekeeper migrator runs in an isolated `lakekeeper` schema

**Files:** `crates/egret-catalog-engine/src/pool.rs`; test `crates/egret-catalog-engine/tests/migrate.rs`.

- [ ] **Step 1:** Implement a helper that builds a lakekeeper Postgres pool whose connections run with `search_path=lakekeeper` (so unqualified `CREATE TABLE` from lakekeeper's migrations land in schema `lakekeeper`, and its `_sqlx_migrations` is `lakekeeper._sqlx_migrations`). Use `PgConnectOptions` with `.options([("search_path", "lakekeeper")])` (or `after_connect` running `SET search_path = lakekeeper`). Create the schema first: `CREATE SCHEMA IF NOT EXISTS lakekeeper`.
- [ ] **Step 2:** Run lakekeeper's migrator on that pool. Find the migrator entry in `lakekeeper-storage-postgres` (the `migrations` module / a `migrate(pool)` fn — read its lib.rs). Port the call lakekeeper-bin's `main.rs` uses.
- [ ] **Step 3 (GATE):** Integration test `migrate_isolates_schemas` (DB-gated, self-skip without `DATABASE_URL`): run egret's migrator (`public`) AND lakekeeper's (`lakekeeper`); assert (a) both succeed, (b) `SELECT to_regclass('lakekeeper."table"')` is non-null, (c) `SELECT to_regclass('public.catalog_tables')` still non-null (egret's untouched), (d) there are TWO independent `_sqlx_migrations` (`public._sqlx_migrations` and `lakekeeper._sqlx_migrations`). Run: `cargo test -p egret-catalog-engine --test migrate`.
- [ ] **Step 4:** Commit `feat(catalog-engine): isolated lakekeeper migrator in its own pg schema`.

**Fallback (record if used):** if `search_path` isolation fights lakekeeper (e.g. it hardcodes `public` or uses cross-schema refs), use a **separate Postgres database** for the catalog (second DSN/pool) and adjust the test accordingly.

---

## Task 3: Build state + mount the Iceberg REST router (AllowAll) + provision one warehouse

**Files:** `crates/egret-catalog-engine/src/lib.rs`, `src/mount.rs`; wire into `crates/egret-server/src/lib.rs`.

- [ ] **Step 1:** Port from `lakekeeper-bin/src/serve.rs` + `main.rs`: build the lakekeeper state `S` — `CatalogState::from_pools(read_pool, write_pool)` (both on the `lakekeeper` schema), `AllowAllAuthorizer`, a postgres `SecretsState` (simplest backend for the spike), and one S3 storage profile from egret config (point at MinIO). Assemble the `ApiContext<S>` the router needs. Document the exact trait bounds `S` had to satisfy (this is a primary spike finding).
- [ ] **Step 2:** Build the router via `lakekeeper::api::iceberg::new_v1_full_router::<...>()` (fill the generics from its signature at `api/iceberg/mod.rs:46`) and expose `pub fn catalog_engine_router(state) -> axum::Router`. Mount it under `/catalog` in egret-server, REPLACING the existing egret `iceberg_router` mount (guard behind a config flag `catalog.engine = "lakekeeper"` so the old one can still build during transition — but default to lakekeeper for this branch).
- [ ] **Step 3:** Provision a **default project** + **one warehouse** at startup (port lakekeeper-bin's bootstrap + create-warehouse calls). For the spike, a single static warehouse named e.g. `spike` pointing at a MinIO bucket is fine (per-workspace warehouses are CE-2).
- [ ] **Step 4 (GATE):** `cargo build --workspace` clean; `cargo clippy -p egret-catalog-engine -p egret-server --all-targets -- -D warnings`. (Behavioral proof is Task 4.)
- [ ] **Step 5:** Commit `feat(catalog-engine): mount lakekeeper iceberg router (AllowAll) + spike warehouse`.

---

## Task 4: Spike acceptance — round-trip a catalog op through lakekeeper

**Files:** `crates/egret-catalog-engine/tests/roundtrip.rs`.

- [ ] **Step 1 (GATE — the spike proof):** DB+MinIO-gated integration test `iceberg_roundtrip_through_lakekeeper` (self-skip without `DATABASE_URL`): build the mounted router with a `tower` oneshot harness (as egret's `http_roundtrip.rs` does), then against the `spike` warehouse:
  1. `GET /catalog/v1/config?warehouse=spike` → 200 with overrides.
  2. `POST /catalog/v1/{prefix}/namespaces` create namespace `demo` → 200.
  3. `POST /catalog/v1/{prefix}/namespaces/demo/tables` create table `t` (minimal schema) → 200.
  4. `GET /catalog/v1/{prefix}/namespaces/demo/tables/t` load → 200, schema round-trips.
  (Use the exact `{prefix}` shape lakekeeper expects — discover it from `new_v1_full_router`'s routes; lakekeeper uses the warehouse in the prefix.)
- [ ] **Step 2:** Run: `cargo test -p egret-catalog-engine --test roundtrip`. Expected: PASS — data persisted/loaded through lakekeeper's store in the `lakekeeper` schema.
- [ ] **Step 3:** Commit `test(catalog-engine): iceberg round-trip through embedded lakekeeper`.

---

## Task 5: Findings report (the real CE-1 deliverable for planning CE-2..8)

**Files:** append a "## CE-1 findings" section to the spec doc, or a new `docs/superpowers/specs/2026-06-26-catalog-engine-CE1-findings.md`.

- [ ] Write up, concretely: which dep mode (git/path) + rev; the single-iceberg confirmation; how schema isolation was achieved (search_path vs separate DB); the exact `ApiContext<S>` trait bounds + which lakekeeper backends are mandatory; the `new_v1_full_router` generics; the `{prefix}`/warehouse routing shape; how a warehouse is created (the API for CE-2); how `AllowAllAuthorizer` is swapped for a custom `Authorizer` (the trait surface for CE-4); where storage profiles plug in (CE-5); where the task/GC workers start (CE-6). These answers de-risk and concretize CE-2..8.
- [ ] Commit `docs(catalog-engine): CE-1 spike findings`.

---

## Self-review checklist
- [ ] Single `iceberg` version confirmed (`cargo tree -d`).
- [ ] Two isolated `_sqlx_migrations` (egret `public`, lakekeeper `lakekeeper`); egret's catalog tables untouched.
- [ ] Router mounts; round-trip (config→create ns→create table→load) passes through lakekeeper's store.
- [ ] `cargo build --workspace` + clippy clean.
- [ ] Findings report written (the input to CE-2..8 plans).

## Out of scope (later CE specs)
Per-workspace warehouses + provisioning (CE-2); token authn + per-token scope (CE-3); custom `Authorizer` bridge (CE-4); storage profiles/secrets bridge (CE-5); GC/task workers (CE-6); UI/internal-API rewire (CE-7); retiring egret's MVP catalog schema/handlers (CE-8).
