# REST Catalog Redesign — Design

> ⛔ **SUPERSEDED (2026-07-02):** replaced by the control-plane Polaris direction — see `docs/en/specs/2026-07-02-control-plane-polaris-catalog-design.md`. The embedded-lakekeeper approach here is dropped (Plans 1 & 4 void; the `write_table` helper is kept). Retained for history.

> Bilingual: English canonical; VI reference at `docs/vi/specs/2026-07-01-rest-catalog-redesign-design.md`.

- **Date:** 2026-07-01 · **Revised:** 2026-07-02 (single-catalog + engine-write pivot)
- **Status:** Approved — write path spike-validated (1a/1b, 2026-07-02)
- **Scope:** Simplify egret's Iceberg REST catalog to a **single, engine-facing catalog** and route notebook writes **through the query engine** (Daft `write_sql`), so the notebook never touches the REST catalog directly. This collapses the previous two-catalog / per-user-JWT / vended-STS design: the catalog needs only **opaque, table-level** auth, and data authorization is enforced **once**, at the engine (Ranger/ACL), for reads and writes alike. Also fixes the residual catalog-plumbing bugs (base_url, config overload) and removes the legacy hand-rolled catalog.

## Background / current state

egret embeds **lakekeeper** (git-pinned `0.12.3`, rev `791650571…`) in-process, mounted at **`/catalog/v1`** (`AppState.catalog_engine`, fronted by `egret-catalog-engine::front_auth`). `egret-catalog::CatalogStore` is the identity layer (opaque `catalog_tokens` + `workspace_warehouses` → lakekeeper-UUID mapping). `front_auth` is hybrid: a bearer JWT (aud=egret-catalog) goes through `CatalogJwtAuth` (F2-P2, the same OIDC delegation Trino/StarRocks use); anything else is an opaque `catalog_tokens` lookup. A legacy hand-rolled REST surface (`egret-catalog::api`, `egret-iceberg-ext`) predates the embed and is superseded.

Until this revision, the notebook read governed data via `trino()`/`starrocks()` but **wrote** via Daft-direct Iceberg (`write_iceberg()`/`iceberg()`/`catalog()`), which required the notebook to hold a catalog credential + storage access — the source of most of the complexity below.

### The residual plumbing bugs (still fixed here)

1. **Internal base URL leaks (`:8181`).** lakekeeper advertises its hardcoded internal bind address in `/config` `overrides.uri` (and per-table signer URLs). Clients — **including the Trino Iceberg REST connector** — follow it and cannot reach `:8181`. PR #114 rewrites the response as a stop-gap. *This still matters after the pivot: Trino follows `overrides.uri`.*
2. **`config.catalog.public_url` is overloaded** — it feeds both the catalog base URL and the kernel callback `api_url`; setting one breaks the other.
3. **Legacy dual catalog code** (`egret-catalog::api` + `egret-iceberg-ext`) coexists with lakekeeper.

## The pivot — single catalog, writes through the engine

**Decision (2026-07-02):** the notebook does not touch the Iceberg REST catalog directly. It **reads** via `trino()`/`starrocks()` and **writes** via a `write_table()` helper that wraps Daft's `df.write_sql(...)` over the same per-user engine connection. There is **one shared catalog** (what Trino/StarRocks read and write); the notebook is just another engine client.

**Why:** it dissolves the hardest complexity (per-user catalog JWTs, vended STS, a private notebook catalog, credential separation) and — because writes also go through the engine — makes **data authorization uniform**: one Ranger/ACL policy governs reads *and* writes; there is no second (Daft-direct) path that could diverge. The earlier "governed data is engine-only" invariant is now automatic, not something to enforce with credential tricks.

**Validated (spike, 2026-07-02, both passed live):**
- **1a:** `df.write_sql` → dev Trino `memory` connector (JWT/TLS) → CREATE + INSERT → read back.
- **1b:** `df.write_sql` → dev Trino **Iceberg REST catalog → lakekeeper → MinIO** → commit → read back.

**Trade-off (accepted):** `write_sql` materializes each partition via `pandas.to_sql`; it suits **EDA-scale** outputs (summaries, feature tables), not bulk loads. It is early-in-development in Daft. Large persistence is a pipeline job, not a notebook job.

## Governance model

- **One enforcement plane: the engine.** Trino/StarRocks + Ranger (or Trino file-ACL) enforce column/row/table policy on every read and every write. The notebook never bypasses it because it has no direct catalog/storage path.
- **The REST catalog is not a governance surface for users.** It authenticates the engine's *service* credential (opaque, table-level) and admin/warehouse-management. Per-user policy lives entirely at the engine.
- **Approach C (catalog-native FGAC, Polaris [#137](https://github.com/apache/polaris/issues/137))** remains a future note only — with engine-only reads/writes there is no gap it needs to close today.

## Credential model

| Plane | Credential | Verified by | Scope |
|---|---|---|---|
| Notebook read (`trino`/`starrocks`) | per-user JWT `aud=trino` / `aud=starrocks` | engine (Keycloak JWKS) + Ranger/ACL | governed reads |
| Notebook write (`write_table`) | the **same** per-user engine JWT | engine + Ranger/ACL | governed writes |
| Engine → catalog (service) | **opaque catalog token, table-level** | egret `front_auth` | the shared warehouse |
| Admin / warehouse-mgmt | opaque catalog token | egret `front_auth` | per warehouse |

The per-user `aud=egret-catalog` JWT path is **no longer used by the notebook** (it may remain in `front_auth` for other callers, but the notebook doesn't need it).

## Architecture / components (revised)

### A. base_url at the source (fixes #1) — `egret-catalog-engine`, lakekeeper fork — **KEEP**
Add `RequestMetadata::with_base_url` to egret's pinned lakekeeper fork; set `LAKEKEEPER__BASE_URI` + stamp the configured public catalog URL on the injected metadata; remove the PR #114 response-rewrite. Still required because the **Trino Iceberg REST connector follows `overrides.uri`** (spike used the rewrite to give Trino a reachable `host.docker.internal` URL). See Plan 1.

### B. Config split (fixes #2) — `egret-config`, `egret-bin` — **KEEP**
Separate `config.catalog.public_url` (catalog URL, for engines/admin) from a new `config.server.public_url` (kernel `api_url`, default `http://127.0.0.1:{port}`). See Plan 1.

### W. Write path — `kernel-image` — **DONE (this revision)**
`write_table(df, "catalog.schema.table", *, engine="trino", mode)` wraps `df.write_sql` over the shared `_trino_conn`/`_starrocks_conn` factory (per-user JWT). It parses the qualified name, sets it as the connection's default catalog/schema (Daft's `SQLDataSink` passes the bare name to `pandas.to_sql`), and `CREATE SCHEMA IF NOT EXISTS` first. Kernel image gains `pandas` (write_sql backend) and `trino` (SQLAlchemy dialect + JWTAuthentication — was referenced but missing). Committed + spike-validated. `engine="starrocks"` is structured but not yet validated.

### R. Remove Daft-direct catalog access — `kernel-image`, `egret-notebook`
Remove `iceberg()` / `write_iceberg()` / `catalog()` / `_resolve_identifier` from the kernel helpers (+ exports, startup, tests). `build_notebook_runtime` / `provision_notebook_access` stop provisioning a per-user notebook catalog token + `notebook_{user}` namespace (the notebook needs no catalog credential); the runtime manifest keeps only what the engine helpers need. Rewrite the opt-in e2e (`notebook-dataframe.spec.ts`) to use `trino()` + `write_table` instead of `write_iceberg`/`iceberg`.

### C. Single shared catalog — `egret-catalog-engine`, `egret-notebook`
One engine-facing warehouse; **drop the notebook-purpose warehouse concept** (no separate Daft-rw catalog). `front_auth` stays for the opaque-token (service + admin) path; the per-user JWT-catalog path is unused by the notebook. (Multi-warehouse may still exist for unrelated reasons, but there is no notebook-specific selection.)

### F. Remove legacy catalog (fixes #3) — `egret-catalog`, `egret-iceberg-ext`
Keep the identity layer (`CatalogStore` tokens + warehouse mapping); remove the dead hand-rolled REST surface + unused `egret-iceberg-ext` types (verify with `codegraph_impact`).

### Ops prerequisites (documented, not egret code)
- **Register the shared Iceberg catalog in Trino/StarRocks** (REST connector → lakekeeper with an opaque service token + S3/MinIO creds — see the spike recipe).
- **Trino JWT → group provider**: dev Trino file-ACL denied writes because Trino has no mapping from the JWT to the user's groups (`tester`'s `data-team` group was unknown → read-only). A group provider (or Ranger) is required for governed writes.

## Data flow (notebook EDA session, after the pivot)

1. **Read governed data:** `trino("SELECT … FROM lakehouse.sales")` → per-user `aud=trino` JWT → Trino + Ranger/ACL → a governed Daft DataFrame.
2. **Write EDA output:** `write_table(df, "lakehouse.analytics.summary", mode="overwrite")` → `CREATE SCHEMA IF NOT EXISTS` → `df.write_sql` over Trino (same JWT) → Trino CREATE TABLE + INSERT into the Iceberg catalog (lakekeeper), data files to S3 — governed by the same policies as the read.
3. **Share:** another user reads it via `trino("SELECT … FROM lakehouse.analytics.summary")` → engine + Ranger/ACL.
4. Trino → lakekeeper uses the opaque **service** token; egret's base_url fix ensures Trino follows a reachable catalog URL.

## Testing

- **Unit (kernel, done):** `write_table` qualifies `catalog.schema.table`, passes the bare name to `write_sql`, ensures the schema, dispatches trino/starrocks, and rejects non-3-part names.
- **Unit (backend):** config split; `with_base_url`.
- **e2e (opt-in, live):** rewrite `notebook-dataframe.spec.ts` to `write_table` → read-back through `trino()`; keep the reopen-and-re-run guard (now trivially green — no per-kernel catalog token to revoke).
- **Spike recipe (dev):** recorded in the redesign memory `egret-catalog-single-catalog-pivot` (Trino REST catalog → lakekeeper via `host.docker.internal`, opaque service token, MinIO S3) — to become the e2e setup.
- **Removed:** the Daft-direct `write_iceberg`/`iceberg`/`catalog` helper tests; the PR #114 rewrite tests (with Plan 1).

## Dependencies

Kernel image: add `pandas` (Daft `write_sql` → `pandas.to_sql`) and `trino` (SQLAlchemy dialect + `JWTAuthentication`; previously referenced in a comment but not installed).

## Out of scope

- Approach C (catalog-native FGAC) — future note only.
- Bulk/large writes from the notebook (EDA-scale via `write_sql` only; large persistence is a pipeline).
- The engine-side Iceberg catalog registration + Ranger/group-provider configuration (operational; documented).

## Risks / notes

- **`write_sql` maturity** — early-in-development in Daft; pin behavior against the installed Daft and re-verify on bump.
- **EDA-scale write ceiling** — `pandas.to_sql` per partition; fine for summaries/feature tables, not bulk.
- **base_url still required** — the Trino Iceberg REST connector follows `overrides.uri`; Plan 1 (A) is not optional.
- **Trino group-mapping** is a hard prerequisite for governed writes (dev lacks it; spike relaxed the ACL temporarily).
- **StarRocks write** is unvalidated (external-catalog selection differs); prefer `engine="trino"` until spiked.
- **Sequencing (revised):** W (write helper — done) → A (base_url) + B (config) → R (remove Daft-direct + rewrite e2e) → C (single-catalog cleanup) → F (legacy removal). Plans **P2 (vended STS)** and **P4 (notebook-JWT)** are **dropped**; **P3 (warehouse purpose)** is **superseded** by C.
