# Catalog Engine = embedded lakekeeper (thin embed) — Design

**Date:** 2026-06-26
**Status:** Design — pending review
**Supersedes (partly):** spec A's catalog *storage/REST engine* (A-1). Keeps A-2/A-3's auth, RBAC, UI, token, provisioning layers (rewired).

---

## 0. Context & decision

Egret is becoming a platform where **each workspace is a self-contained data platform** ([[egret-platform-vision]]). The Catalog pillar must be production-grade and usable standalone with every Iceberg engine. Rather than reimplement lakekeeper's operational machinery (GC, soft-delete, multi-storage, full REST, statistics), egret will **embed lakekeeper's core as a library (thin embed)**.

**Feasibility (verified):**
- egret and lakekeeper pin the **same `iceberg-rust` fork rev** (`bb70173`) → no `iceberg` type conflict. This is the enabling fact.
- lakekeeper is **license Apache-2.0** and **deliberately embeddable**: `pub mod service` (traits `CatalogStore`/`Authorizer`/`SecretStore`, ids `ProjectId`/`WarehouseId`/`SecretId`), `pub mod api` (`ApiContext<S: ThreadSafe>` generic router state), `pub mod serve` (feature `router`), re-exports `axum`/`iceberg`. Ships an `AllowAllAuthorizer` and `lakekeeper-storage-postgres` (a ready `CatalogStore` backend + 63-file migrator). `lakekeeper-bin` is the canonical embedding example.

**Thin embed** (decided): lakekeeper = the **catalog engine** (store + REST handlers + GC tasks + storage I/O). egret keeps its **identity layer**: per-workspace token authn, RBAC, Catalog UI, warehouse provisioning, and a custom `Authorizer` bridge carrying **per-token scopes**. Tenancy: one lakekeeper **default project**, **one warehouse per egret workspace** (`{prefix}` = workspace slug → warehouse).

**Why thin (not thick):** egret owns identity/RBAC/UI/token-scoping natively; lakekeeper owns catalog mechanics. Every product requirement raised (per-token scope, workspace RBAC, egret-native UI) needs egret to own authz — which thin embed preserves.

---

## 1. Target architecture

```
egret-server (axum)
  /catalog  ──►  lakekeeper::api router  (ApiContext<S>)
                   │  state S provides:
                   │    • CatalogStore   = lakekeeper-storage-postgres (lakekeeper schema, separate PG schema)
                   │    • Authorizer     = EgretAuthorizer (bridge → workspace RBAC + per-token scope)   [CE-4]
                   │    • SecretStore     = bridge → egret-secrets (or lakekeeper postgres secrets)        [CE-5]
                   │    • Storage (io)    = lakekeeper-io storage profiles (S3 now; GCS/Azure later)        [CE-5]
                   │    • Task queues     = lakekeeper GC/expire/soft-delete workers (in-process)           [CE-6]
                   ▼
   egret middleware in front: require_bearer (catalog_tokens → principal + scope)                         [CE-3]
   tenancy: 1 default project; 1 warehouse / workspace; {prefix}=slug → warehouse                          [CE-2]
   provisioning: workspace create → create lakekeeper warehouse (rewire A-2 hook)                          [CE-2]
   UI: egret Catalog browser + internal API read lakekeeper data                                            [CE-7]
```

**Fate of spec A:**
- **Replaced:** `catalog_*` schema + `store.rs`/`table_repo.rs`/`view_repo.rs` + Iceberg REST handlers (`api/*`) + `resolve.rs` + migration 0023 catalog tables. → lakekeeper provides these (and GC/multi-storage/statistics on top).
- **Kept & rewired:** A-3 token auth (`catalog_tokens` + `require_bearer`) → authn bridge; A-3 RBAC perms (`catalog:read|write`) → feed `EgretAuthorizer`; A-3 internal browse API + Catalog UI → re-point to lakekeeper data; A-2 provisioning hook → create warehouse. A-1/A-2/A-3 were the right scaffolding; ~60% survives.

---

## 2. Decomposition (each = its own spec→plan→build)

| # | Sub-project | Outcome |
|---|-------------|---------|
| **CE-1** | **Spike + foundation** (this spec) | lakekeeper builds as an egret dep; its migrator runs in a **separate PG schema**; a minimal lakekeeper router mounts under `/catalog` with `AllowAllAuthorizer`; one real Iceberg REST call (`/v1/config` + create namespace) succeeds end-to-end. De-risks the trait/mount/migrator unknowns. |
| CE-2 | Tenancy + provisioning | default project; workspace-create → lakekeeper warehouse; slug→warehouse resolution. |
| CE-3 | Authn bridge + per-token scope | `catalog_tokens` + `permissions[]` + `namespace_scope[]`; token → lakekeeper principal carrying scope. |
| CE-4 | Authz bridge | `EgretAuthorizer` impl of lakekeeper's `Authorizer` = workspace RBAC + token scope; enforced before sign. |
| CE-5 | Storage profiles + secrets | per-workspace storage (S3 now; GCS/Azure later) via lakekeeper-io; secret bridge. |
| CE-6 | GC / task workers | run lakekeeper expiration/soft-delete/orphan-cleanup; retention config per workspace. |
| CE-7 | UI + internal API rewire | browse API + Catalog UI read lakekeeper data (richer: snapshot history, stats). |
| CE-8 | Retire MVP catalog | drop superseded `catalog_*` schema + egret store/handlers; keep auth/RBAC/UI/token/provisioning. |

---

## 3. CE-1 detailed design (spike + foundation)

**Goal:** prove the embed is mechanically sound and establish the wiring the rest builds on. CE-1 is a working vertical slice, NOT throwaway — but with deliberately deferred pieces (`AllowAllAuthorizer`, static single warehouse).

### 3.1 Dependencies
- Add to egret workspace `Cargo.toml`: `lakekeeper`, `lakekeeper-storage-postgres`, `lakekeeper-io` as **git deps pinned to a rev** (lakekeeper is a separate repo; pin a rev for reproducibility — path dep only if co-developing). Enable features `router` (+ `open-api` optional). Confirm the shared `iceberg` rev `bb70173` resolves to ONE version (no duplicate `iceberg` in `cargo tree`).
- New crate `egret-catalog-engine` (or a module in egret-server) that owns the lakekeeper wiring, so the embed surface is one focused unit.

### 3.2 Migrator coexistence (the key risk)
lakekeeper-storage-postgres runs 63 migrations via its own `sqlx::migrate!` using the default `_sqlx_migrations` table — egret-metastore uses the same table. Running both against one DB/schema collides.
**Approach:** run lakekeeper's catalog schema in a **dedicated Postgres schema** (e.g. `lakekeeper`): set `search_path`/schema on lakekeeper's pool, or create the schema and point lakekeeper's migrator at it, so its `_sqlx_migrations` + tables live under `lakekeeper.*`, isolated from egret's `public.*`. Verify lakekeeper-storage-postgres supports a configurable schema (check `CatalogState` / connection setup); if it hard-codes `public`, **fallback:** a separate Postgres database for the catalog, with a second pool. CE-1 must settle this concretely and document which path works.

### 3.3 Router mount (AllowAll first)
- Build `ApiContext<S>` with: `CatalogStore` = lakekeeper-storage-postgres `CatalogState` (its pool on the `lakekeeper` schema), `Authorizer` = `AllowAllAuthorizer`, `SecretStore` = lakekeeper postgres secrets (simplest for CE-1), storage = a single S3 profile from egret config.
- Mount the lakekeeper `api` router under egret-server `/catalog` (replacing the egret iceberg_router mount). Follow `lakekeeper-bin`'s router-build code as the template.
- Auth: keep egret `require_bearer` as a thin front layer for CE-1 (static token ok), real token→principal bridge is CE-3.

### 3.4 Acceptance (the spike proof)
DB-gated integration test (self-skip without `DATABASE_URL`):
1. Run both migrators (egret `public`, lakekeeper `lakekeeper` schema) cleanly.
2. Provision one lakekeeper project (default) + one warehouse (static, pointing at a test S3/MinIO or in-memory).
3. Drive the mounted router: `GET /catalog/v1/config?warehouse=<wh>` returns config; create a namespace; create a table; load it back — all 200, data round-trips through lakekeeper's store.
4. Confirm `cargo tree -d` shows a single `iceberg` version.
This proves: deps build, migrator coexistence works, router mounts, lakekeeper's store functions inside egret.

### 3.5 Explicit de-risk fallbacks (record outcomes in CE-1)
- If lakekeeper-storage-postgres can't target a non-`public` schema → use a **separate catalog database** (second pool/DSN).
- If a feature flag / trait bound is heavier than expected → document the exact `ApiContext` state bounds and which lakekeeper backends are mandatory.
- If `iceberg` rev resolves to two versions → align egret's pin or vendor.

---

## 4. Risks / open questions
- **Migrator/schema coexistence** (CE-1 settles).
- **Warehouse-per-workspace at scale** — many warehouses in one lakekeeper project; confirm acceptable (lakekeeper is built for many warehouses, so fine).
- **Per-token scope vs lakekeeper vended-credentials/sign** — egret's `Authorizer` (CE-4) must deny before lakekeeper signs S3 (mirror A-3's "deny before sign").
- **Upgrade coupling** — pinned lakekeeper rev; bumping = a deliberate, tested step.
- **Data migration of existing egret catalog rows** — pre-release: drop & recreate (no real data), per A-1's precedent (CE-8).

## 5. Testing
- CE-1: the spike integration test above + `cargo tree -d` single-iceberg check + clippy/fmt.
- Later CEs: per-engine REST conformance (Trino/Spark/Daft/PyIceberg), authz deny-paths, GC behavior.

## 6. Out of scope (other epic parts / deferred)
- lakekeeper projects, OpenFGA, events, contract-verification (egret has equivalents or doesn't need).
- Notebook, storage/file-browser, Engineering pillar — separate epics.
- GCS/Azure storage (CE-5 may stub; S3 first).
